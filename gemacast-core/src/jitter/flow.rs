//! Playback-lifecycle actor: owns the state machine that tracks whether we are
//! prebuffering, starving, or holding for a reordered packet, plus the NetEQ
//! IIR-filtered buffer level. Owns no jitter buffer or decoder — the orchestrator
//! reads these counters to sequence prebuffer / gap / play / starve transitions.

use std::time::{Duration, Instant};

/// Wall-clock ceiling on the starvation-recovery guard.
///
/// The `starvation_recovery` countdown is refreshed on *every* recovery, so a
/// starvation cluster arriving closer together than the countdown is long holds
/// the guard open indefinitely: one 5GHz field capture ran 9.4s with ~248
/// starvation events and never re-enabled either accelerate or WSOLA expand.
/// WebRTC suppresses time-stretching for exactly one frame after an expand
/// (`prev_mode != kModeExpand`, `decision_logic.cc:285`), so 500ms is still 50x
/// more conservative than upstream — and a buffer that is starving needs
/// `expand` more than it needs protection from `expand`.
const RECOVERY_GUARD_WINDOW: Duration = Duration::from_millis(500);

/// Length of the concealment-ratio window, in callbacks (500ms at 10ms frames).
///
/// A *tumbling* window: both counters are zeroed at every boundary, so the
/// measure ages out completely and cannot latch its own history.
const CONCEAL_WINDOW_CALLBACKS: u32 = 500 / crate::jitter::consts::MILLIS_PER_FRAME;

/// Share of a window that must be concealed for the playhead to be held.
///
/// Measured, not guessed. In a 5GHz starvation storm one packet landed roughly
/// every 3rd callback, so ~⅔ of output was PLC — 299 onsets in 9.4s. A
/// healthy link conceals ~0%, and a single DTIM gap large enough to reach 50%
/// of a 500ms window has already tripped `REBUFFER_AFTER` five callbacks in, so
/// this threshold only decides cases that mechanism cannot see.
const CONCEAL_REBUFFER_PCT: u32 = 50;

/// One window's worth of delivery health, produced at each window boundary.
pub(super) struct ConcealVerdict {
    /// Share of the window's callbacks that had no playable frame, 0-100.
    pub conceal_pct: u32,
    /// Callbacks measured — always [`CONCEAL_WINDOW_CALLBACKS`], carried so the
    /// log line states its own denominator.
    pub callbacks: u32,
    /// True when the playhead should be held to bank depth in one hold instead
    /// of stuttering through the deficit.
    pub should_rebuffer: bool,
}

/// Rolling playback-lifecycle state derived from buffer occupancy over time.
pub(super) struct PlaybackFlow {
    /// True while accumulating the initial buffer before playback starts.
    pub is_prebuffering: bool,
    /// Consecutive callbacks with no playable frame (drives the hard-reset timeout).
    pub missing_count: u32,
    /// Consecutive callbacks the buffer has been fully empty (drives rebuffer).
    pub starvation_count: u32,
    /// Consecutive callbacks that emitted concealment instead of a decoded frame.
    ///
    /// Distinct from [`Self::starvation_count`], which is incremented only on the
    /// starvation path and therefore **freezes at `REBUFFER_AFTER`** for the whole
    /// rebuffer hold: once the hold arms, every callback takes the prebuffer early
    /// return, which conceals and returns without touching that counter. This one
    /// is incremented by the concealment generator itself, so it counts what was
    /// actually emitted on both hold and starvation paths and falls to zero the
    /// moment a real frame is played.
    ///
    /// This is upstream's `consecutive_expands_` (`expand.cc:150-312`). Without
    /// it nothing counted how long a concealment run had been going, so the
    /// bound on it was inferred rather than measured.
    pub conceal_run: u32,
    /// How many consecutive callbacks we've been waiting for the current gap slot.
    /// Prevents spurious PLC for late-arriving reordered packets on 2.4GHz.
    pub gap_hold_count: u32,
    /// Starvation recovery countdown, in callbacks. Refreshed on every recovery
    /// to `effective_target * 2` clamped to [50, 200] — i.e. 500ms to 2s, not
    /// the one frame WebRTC suppresses for (`prev_mode != kModeExpand`,
    /// `decision_logic.cc:285`). Its remaining job is to make the `probe_floor`
    /// bump fire once per starvation *cluster* rather than once per event, which
    /// needs the long, re-armable countdown: without it the field log walked the
    /// floor 155→159→163→…→175 inside 300ms.
    ///
    /// It is deliberately **no longer** the time-stretch gate. Because it
    /// re-arms unguarded it cannot fall while starvations keep coming, and a
    /// latch that gates the recovery actuators is a latch that prevents
    /// recovery. [`Self::stretch_allowed`] uses `recovery_started_at` instead.
    pub starvation_recovery: u32,
    /// When the *current* starvation-recovery episode began. Set on the first
    /// arm only and left alone by later starvations, so the suppression window
    /// is bounded by wall clock however many times recovery re-enters. Cleared
    /// by [`Self::tick_recovery`] once [`RECOVERY_GUARD_WINDOW`] has elapsed.
    pub recovery_started_at: Option<Instant>,
    /// NetEQ-style IIR filtered buffer level to ignore instantaneous OS batching spikes.
    pub filtered_buffer_level: f32,
    /// Callbacks concealed (no playable frame) in the current window.
    conceal_callbacks: u32,
    /// Callbacks elapsed in the current window.
    window_callbacks: u32,
    /// Starvation onsets in the current episode. Drives one log line per episode
    /// with a census, instead of one line per onset — a 5GHz storm emitted 299
    /// identical `Starvation started` warnings.
    pub starvation_events: u32,
    /// When the current starvation episode began, or `None` between episodes.
    pub episode_started_at: Option<Instant>,
}

impl PlaybackFlow {
    pub fn new() -> Self {
        Self {
            is_prebuffering: true,
            missing_count: 0,
            starvation_count: 0,
            conceal_run: 0,
            gap_hold_count: 0,
            starvation_recovery: 0,
            recovery_started_at: None,
            filtered_buffer_level: 0.0,
            conceal_callbacks: 0,
            window_callbacks: 0,
            starvation_events: 0,
            episode_started_at: None,
        }
    }

    /// NetEQ IIR Buffer Filter (Method 5).
    /// Heavily low-passes the instantaneous buffer level so that massive batching
    /// (e.g. 10 packets arriving at once via USB) doesn't trigger a spurious drain.
    ///
    /// The smoothing coefficient is **target-driven**, mirroring NetEQ's
    /// `BufferLevelFilter::SetTargetBufferLevel`: a larger target uses a slower
    /// filter (more smoothing), a small target (ADB/5GHz, ~1-3 frames) uses a
    /// faster filter so occupancy is tracked closely and the buffer can settle low.
    /// Coefficients are NetEQ's, expressed as α/256:
    ///   target ≤ 1 → 251/256 ≈ 0.980   target ≤ 3 → 252/256 ≈ 0.984
    ///   target ≤ 7 → 253/256 ≈ 0.988   else       → 254/256 ≈ 0.992
    pub fn filter_buffer_level(&mut self, occupied: u32, target: u32) -> f32 {
        let level_factor = if target <= 1 {
            251.0
        } else if target <= 3 {
            252.0
        } else if target <= 7 {
            253.0
        } else {
            254.0
        };
        let alpha = level_factor / 256.0;
        self.filtered_buffer_level =
            self.filtered_buffer_level * alpha + (occupied as f32) * (1.0 - alpha);
        self.filtered_buffer_level
    }

    /// Immediately correct the filtered level after a WSOLA time-stretch, mirroring
    /// NetEQ's `BufferLevelFilter` time-stretch compensation. Acceleration removes
    /// audio (pass a negative `delta_frames`); preemptive expand inserts audio
    /// (positive). Without this correction the α≈0.99 filter lags the real buffer
    /// change by ~1s, which blinds the drain decision and lets the buffer balloon.
    /// Floored at 0.
    pub fn adjust_filtered_level(&mut self, delta_frames: f32) {
        self.filtered_buffer_level = (self.filtered_buffer_level + delta_frames).max(0.0);
    }

    /// Record one callback's delivery outcome and, at a window boundary, return
    /// the window's verdict.
    ///
    /// This is the counter a starvation storm needed and none of the existing
    /// ones could provide. `starvation_count` counts *consecutive* empty
    /// callbacks and is zeroed by any pop, so under a ⅓-rate trickle — a packet
    /// every ~3rd callback — it never reaches `REBUFFER_AFTER`: one capture
    /// shows 299 starvation onsets producing exactly **one** rebuffer and zero
    /// stream resets across 9.4s. A run-length counter cannot see a deficit that
    /// is interrupted by the very arrivals that constitute it; a ratio can.
    ///
    /// The window tumbles rather than decays, so the measure is recomputed from
    /// scratch every 500ms and cannot latch its own history.
    pub fn observe_delivery(&mut self, concealed: bool) -> Option<ConcealVerdict> {
        self.window_callbacks += 1;
        if concealed {
            self.conceal_callbacks += 1;
        }
        if self.window_callbacks < CONCEAL_WINDOW_CALLBACKS {
            return None;
        }
        let conceal_pct = self.conceal_callbacks * 100 / self.window_callbacks;
        let verdict = ConcealVerdict {
            conceal_pct,
            callbacks: self.window_callbacks,
            should_rebuffer: conceal_pct >= CONCEAL_REBUFFER_PCT,
        };
        self.conceal_callbacks = 0;
        self.window_callbacks = 0;
        Some(verdict)
    }

    /// Abandon the current concealment window without a verdict. Called when the
    /// playhead is already held (rebuffering) or the stream restarts: those
    /// callbacks conceal by construction, and counting them would re-trigger the
    /// hold the moment it ended.
    pub fn discard_conceal_window(&mut self) {
        self.conceal_callbacks = 0;
        self.window_callbacks = 0;
    }

    /// Register a starvation onset. Returns `true` for the first onset of an
    /// episode — the caller logs then, and logs the census when
    /// [`Self::close_starvation_episode`] reports one.
    ///
    /// An episode spans a *cluster*: it stays open across the pops that separate
    /// onsets, and is closed only by [`Self::close_starvation_episode`] once
    /// delivery has actually recovered. This is what collapses a storm's many
    /// identical `Starvation started` warnings into one line — a volume
    /// change only, no loss of coverage, since the census carries the count and
    /// duration that the repeated lines conveyed by their sheer number.
    pub fn note_starvation_onset(&mut self, now: Instant) -> bool {
        self.starvation_events += 1;
        if self.episode_started_at.is_none() {
            self.episode_started_at = Some(now);
            return true;
        }
        false
    }

    /// Close a starvation episode if one is open, returning its onset count and
    /// wall-clock duration for the census log. Idempotent.
    pub fn close_starvation_episode(&mut self, now: Instant) -> Option<(u32, Duration)> {
        let started = self.episode_started_at.take()?;
        let events = std::mem::take(&mut self.starvation_events);
        Some((events, now.duration_since(started)))
    }

    /// Per-callback tick of the starvation-recovery guard.
    ///
    /// `now` is threaded in rather than read here so the audio callback reuses
    /// the single `Instant::now()` it already takes, and so the guard window is
    /// testable without sleeping.
    pub fn tick_recovery(&mut self, now: Instant) {
        self.starvation_recovery = self.starvation_recovery.saturating_sub(1);
        // Drop the window as soon as it expires, so `recovery_started_at` and
        // `stretch_allowed` can never disagree about whether we are guarded.
        if let Some(started) = self.recovery_started_at
            && now.duration_since(started) >= RECOVERY_GUARD_WINDOW
        {
            self.recovery_started_at = None;
        }
    }

    /// Arm the time-stretch suppression window. Idempotent while the window is
    /// open: a second starvation inside it does **not** extend it, which is the
    /// whole point — the old countdown was refreshed on every recovery and so
    /// never expired under a starvation cluster.
    pub fn arm_recovery_window(&mut self, now: Instant) {
        if self.recovery_started_at.is_none() {
            self.recovery_started_at = Some(now);
        }
    }

    /// Whether WSOLA accelerate / preemptive expand may run this callback.
    ///
    /// False only inside [`RECOVERY_GUARD_WINDOW`] of the first starvation of the
    /// current episode. This keeps the original saw-tooth protection (don't drain
    /// a buffer that just emptied) while guaranteeing the actuators come back.
    pub fn stretch_allowed(&self, now: Instant) -> bool {
        match self.recovery_started_at {
            Some(started) => now.duration_since(started) >= RECOVERY_GUARD_WINDOW,
            None => true,
        }
    }

    /// Partial reset on stream restart (matches the legacy `trigger_reset` field set):
    /// re-enters prebuffering and zeroes the missing/starvation/gap counters.
    ///
    /// `filtered_buffer_level` is snapped to zero rather than left to coast: the IIR
    /// time constant is ~1.3s at a large target, so a stale pre-restart reading
    /// survives well past the recovery guard and misdirects the very first
    /// drain/expand decision after the stream comes back. `starvation_recovery`
    /// and `recovery_started_at` are both deliberately left untouched — a restart
    /// is not a reason to re-enable drain, and the new window expires on its own
    /// within 500ms anyway.
    pub fn reset_on_stream_restart(&mut self) {
        self.is_prebuffering = true;
        self.missing_count = 0;
        self.starvation_count = 0;
        // The run belonged to the stream that just ended. Carrying it across a
        // restart would open the new stream's first concealment already faded.
        self.conceal_run = 0;
        self.gap_hold_count = 0;
        self.filtered_buffer_level = 0.0;
        // The concealment window measured the *old* stream; carrying it across a
        // restart would hold the playhead on evidence that no longer applies.
        self.discard_conceal_window();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The IIR filter must use a slower coefficient for a large target and a
    /// faster one for a small target (NetEQ `SetTargetBufferLevel`). Concretely,
    /// from the same starting state and occupancy, a small target must move the
    /// filtered level further toward the new occupancy in one step.
    #[test]
    fn filter_coefficient_is_target_driven() {
        let mut slow = PlaybackFlow::new(); // large target → α=254/256
        let mut fast = PlaybackFlow::new(); // tiny target → α=251/256
        slow.filtered_buffer_level = 0.0;
        fast.filtered_buffer_level = 0.0;

        // Same occupancy step (100 frames) into both, different targets.
        let slow_level = slow.filter_buffer_level(100, 50);
        let fast_level = fast.filter_buffer_level(100, 1);

        // Faster filter (small target) tracks the jump more aggressively.
        assert!(
            fast_level > slow_level,
            "small-target filter ({fast_level:.3}) should track occupancy faster than large-target ({slow_level:.3})",
        );
    }

    /// A starvation storm as a unit test: starvations recurring faster than the
    /// old `starvation_recovery` countdown (50-200 callbacks) refreshed it forever,
    /// so `stretch_allowed` never returned true and neither accelerate nor expand
    /// could run again for the rest of the stream — 9.4s and ~248 events in the
    /// field log. The window must expire on wall clock no matter how often
    /// recovery re-enters.
    #[test]
    fn sustained_starvation_should_not_hold_the_recovery_guard_open_forever() {
        let mut flow = PlaybackFlow::new();
        let base = Instant::now();

        // 2s of simulated playback: a starvation every 3rd callback, i.e. always
        // well inside the countdown the old gate used.
        let mut allowed_after_window = 0;
        for callback in 0..200u32 {
            let now = base + Duration::from_millis(u64::from(callback) * 10);
            flow.tick_recovery(now);
            if callback % 3 == 0 {
                // What the manager does on recovery from starvation.
                flow.starvation_recovery = 200;
                flow.arm_recovery_window(now);
            }
            if now.duration_since(base) > RECOVERY_GUARD_WINDOW && flow.stretch_allowed(now) {
                allowed_after_window += 1;
            }
        }

        assert!(
            allowed_after_window > 0,
            "time-stretch must be re-enabled once the 500ms window expires, however \
             many starvations re-entered recovery",
        );
        assert!(
            flow.starvation_recovery > 0,
            "the floor-bump cooldown is still legitimately armed — the two guards \
             are independent, and only the stretch gate is wall-clock bounded",
        );
    }

    /// The converse, so the fix does not simply delete the saw-tooth protection:
    /// immediately after a starvation, draining the buffer that just emptied must
    /// still be suppressed.
    #[test]
    fn recovery_guard_should_still_suppress_stretch_immediately_after_one_starvation() {
        let mut flow = PlaybackFlow::new();
        let base = Instant::now();

        assert!(
            flow.stretch_allowed(base),
            "unarmed flow must permit time-stretch",
        );

        flow.arm_recovery_window(base);
        assert!(
            !flow.stretch_allowed(base),
            "suppressed at the instant of arming"
        );
        assert!(
            !flow.stretch_allowed(base + Duration::from_millis(499)),
            "still suppressed just inside the window",
        );
        assert!(
            flow.stretch_allowed(base + Duration::from_millis(500)),
            "released exactly at the window edge",
        );

        // And the state must be dropped, not merely read as expired, so the two
        // fields cannot disagree on a later callback.
        flow.tick_recovery(base + Duration::from_millis(500));
        assert!(
            flow.recovery_started_at.is_none(),
            "expired window is cleared"
        );
    }

    /// A starvation cluster must not extend the window: arming is idempotent while
    /// it is open, which is the single behavioural difference from the countdown it
    /// replaces.
    #[test]
    fn re_arming_inside_the_window_must_not_extend_it() {
        let mut flow = PlaybackFlow::new();
        let base = Instant::now();

        flow.arm_recovery_window(base);
        for ms in [100, 200, 300, 400] {
            flow.arm_recovery_window(base + Duration::from_millis(ms));
        }

        assert_eq!(
            flow.recovery_started_at,
            Some(base),
            "the window keeps its original start, so it expires 500ms after the \
             FIRST starvation of the episode",
        );
        assert!(flow.stretch_allowed(base + Duration::from_millis(500)));
    }

    /// After a WSOLA stretch, the filtered level must be corrected immediately by
    /// the number of frames added/removed — NetEQ's time-stretch compensation.
    /// This is the mechanism whose absence caused the ~1.3s drain lag and the
    /// 2.4GHz plateau.
    #[test]
    fn adjust_filtered_level_debits_and_credits_and_floors_at_zero() {
        let mut flow = PlaybackFlow::new();
        flow.filtered_buffer_level = 10.0;

        // Accelerate removed ~1.5 frames of audio → debit.
        flow.adjust_filtered_level(-1.5);
        assert!((flow.filtered_buffer_level - 8.5).abs() < 1e-6);

        // Expand inserted ~0.5 frames → credit.
        flow.adjust_filtered_level(0.5);
        assert!((flow.filtered_buffer_level - 9.0).abs() < 1e-6);

        // Over-debit must floor at zero, never go negative.
        flow.adjust_filtered_level(-100.0);
        assert_eq!(flow.filtered_buffer_level, 0.0);
    }
}
