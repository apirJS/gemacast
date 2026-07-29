//! Depth-decision actor: owns the adaptive target-depth policy — hysteresis,
//! quantization, rate-limited ramping, downward probing, and the post-starvation
//! bump. Reads [`super::stats::JitterStats`] and [`JitterConfig`] as inputs; owns
//! no buffer or decoder. The orchestrator feeds it the raw computed target each
//! callback and receives the smoothed effective target back.

use std::collections::VecDeque;
use std::time::Instant;

use super::consts::ms_to_frames_ceil;
use super::stats::JitterStats;
use crate::domain::types::JitterConfig;

/// Hysteresis half-width in frames. The effective target only moves when the
/// raw computed target deviates by more than this many frames.
const HYSTERESIS_BAND: u32 = 3;
/// Snap effective target to multiples of this many frames to reduce
/// the total number of discrete target transitions.
const TARGET_QUANTUM: u32 = 4;
/// Rate-limit interval: effective target moves DOWN by at most 1 frame every
/// this many callbacks, smoothing transitions for artifact-free playback.
const RAMP_INTERVAL: u32 = 5;
/// Frames per step when climbing, and callbacks between steps: ~100 frames/s of
/// target growth. Faster than any real link degradation, yet slow enough that
/// `low_limit` never outruns the actual buffer by more than a frame or two.
///
/// The old behaviour — jump straight to `ramp_goal` in one callback — is what
/// produced the "fast clicking on every buffer increase": a 15-frame jump puts
/// `low_limit` 11 frames above the real buffer level, and the preemptive-expand
/// path then fires a single-pitch-period OLA splice every cooldown window for
/// several seconds straight.
const RAMP_UP_STEP: u32 = 2;
const RAMP_INTERVAL_UP: u32 = 2;
/// When the network has been stable for a sustained period, try probing
/// lower every this many callbacks. One quantum step down per probe.
const PROBE_DOWN_INTERVAL: u32 = 200;

/// How long to track starvation events for rate limiting probes.
const STARVATION_WINDOW_SECS: u64 = 30;
/// Probe interval when starvation occurred recently — 6 seconds between
/// probe steps instead of the normal 300-600ms. Prevents the
/// probe-down→starve→bump-up→probe-down oscillation.
const PROBE_GATED_INTERVAL: u32 = 600;

/// Starvation-free seconds before the learned floor begins to relax.
const FLOOR_HOLD_SECS: u64 = 8;
/// Seconds between successive one-quantum relaxations of the learned floor.
/// At 2s a floor sitting at the `cap/2` ceiling (40 frames on 2.4GHz) is fully
/// relaxed in 8 + 10×2 ≈ 28s — inside the screen-on snap-back budget. At the
/// previous 4s it took ~48s, which alone blew that budget.
const FLOOR_DECAY_SECS: u64 = 2;

/// Adaptive target-depth controller. Holds all hysteresis, ramp, probe, and
/// starvation-bump state; produces the smoothed effective target each callback.
pub(super) struct TargetController {
    /// The currently locked-in effective target depth (frames). Only moves when
    /// the raw computed target exits the hysteresis band for the dwell period.
    pub effective_target: u32,
    /// How many consecutive callbacks the raw target has been outside the band.
    pub target_exit_count: u32,
    /// The quantized goal that `effective_target` is ramping toward.
    pub ramp_goal: u32,
    /// Countdown for rate-limited ramping (one step per RAMP_INTERVAL callbacks).
    ramp_countdown: u32,
    /// Countdown for active downward probing when the network is stable.
    probe_down_countdown: u32,
    /// Learned floor: the lowest effective_target that caused starvation.
    /// Probing won't go below this. Bounded at `cap/2` and relaxed on a timer,
    /// so it can never be the thing that pins the target at the comfort cap.
    probe_floor: u32,
    /// Recent starvation timestamps for rate-limiting probe aggressiveness.
    starvation_timestamps: VecDeque<Instant>,
    /// When the most recent starvation was recorded, for the floor-relax hold.
    last_starvation: Option<Instant>,
    /// When the floor was last relaxed by one quantum.
    last_floor_decay: Instant,
}

impl TargetController {
    pub fn new() -> Self {
        Self {
            effective_target: 2,
            target_exit_count: 0,
            ramp_goal: 2,
            ramp_countdown: 0,
            probe_down_countdown: PROBE_DOWN_INTERVAL,
            probe_floor: 0,
            starvation_timestamps: VecDeque::new(),
            last_starvation: None,
            last_floor_decay: Instant::now(),
        }
    }

    /// Minimum buffer depth in frames, from config.
    fn min_depth_frames(config: &JitterConfig) -> u32 {
        ms_to_frames_ceil(config.min_depth_ms)
    }

    /// Comfort cap in frames, from config.
    fn comfort_cap_frames(config: &JitterConfig) -> f32 {
        ms_to_frames_ceil(config.comfort_cap_ms) as f32
    }

    /// The learned starvation floor, in frames. Read-only: the floor may only be
    /// raised by `apply_starvation_floor` and lowered by its own timed decay, so
    /// exposing it for logging (and for asserting "one gap costs one floor bump")
    /// must not hand out a way to set it.
    pub fn probe_floor(&self) -> u32 {
        self.probe_floor
    }

    /// Snap a raw target to the nearest quantum level.
    /// Reduces the total number of discrete target transitions.
    fn quantize_target(raw: u32, quantum: u32) -> u32 {
        if quantum == 0 {
            return raw;
        }
        ((raw + quantum / 2) / quantum) * quantum
    }

    /// Adaptive quantum: step size for target transitions, scaled to the comfort
    /// cap so a preset always has ~16 discrete levels to settle on.
    ///
    /// Derived from the cap rather than branching on thresholds: the old
    /// `cap <= 8 → 1 / cap >= 100 → 2 / else 4` ladder was **non-monotone**, so
    /// an 800ms cap got coarser steps (4) than a 1000ms cap (2).
    fn adaptive_quantum(config: &JitterConfig) -> u32 {
        let cap = Self::comfort_cap_frames(config) as u32;
        (cap / 16).clamp(1, TARGET_QUANTUM)
    }

    /// Adaptive hysteresis: dead-zone half-width, scaled to the comfort cap.
    /// Narrow for low-cap presets so the target can distinguish between 1, 2 and
    /// 3 frames instead of treating them all as within the dead-zone.
    fn adaptive_hysteresis(config: &JitterConfig) -> u32 {
        let cap = Self::comfort_cap_frames(config) as u32;
        (cap / 8).clamp(1, HYSTERESIS_BAND)
    }

    /// Adaptive dwell time: how many callbacks the raw target must stay
    /// outside the hysteresis band before committing to a new goal.
    /// High-cap presets (Auto, Resilient) react faster since they have
    /// more headroom. Low-cap presets stay conservative.
    fn adaptive_dwell(config: &JitterConfig) -> u32 {
        let cap = Self::comfort_cap_frames(config) as u32;
        40u32.saturating_sub((cap / 2).min(25)).max(15)
    }

    /// Pure computation of the target buffer depth from observed jitter statistics.
    pub fn compute_target_depth(
        &self,
        config: &JitterConfig,
        stats: &JitterStats,
        tcp_cap_override: Option<f32>,
    ) -> u32 {
        // Static mode: lock buffer to exact user-specified depth, bypass all adaptive math.
        // min_depth is intentionally not applied — the user's explicit target is authoritative.
        if let Some(static_ms) = config.static_target_ms {
            return ms_to_frames_ceil(static_ms);
        }
        // NetEQ's base target: the 95th percentile of the relative arrival delay
        // histogram. Since v5 this is fed NetEQ-style (running sum of per-packet
        // IAT excess over a 100-packet history), so a DTIM gap shows up in every
        // packet of the burst that follows it rather than as a single outlier.
        let histogram_base = stats.iat_percentile_target;
        let burst_floor = if stats.burst_detected() {
            stats.inter_burst_gap_frames() + 2.0
        } else {
            0.0
        };
        // Worst delivery gap in the recent window, plus headroom.
        // This is the primary depth signal on DTIM-batched links, where
        // per-packet jitter is ~0.05 frames but the inter-cluster gap is 20-60
        // frames — precisely the blind spot of a p95-of-packets statistic like
        // `histogram_base`. Unlike every other term here it needs no starvation
        // to learn the gap, and it ages out on its own.
        let gap = stats.max_gap_frames();
        let gap_floor = if gap > 1.0 {
            if stats.burst_detected() {
                // +25% while DTIM batching is detected. Doze gaps *grow* as the
                // radio sleeps deeper (the field log walks 21 → 48 → 106 frames),
                // and a purely reactive floor loses to a growing gap by exactly
                // one starvation every time. The headroom absorbs the next step
                // of the growth instead of stuttering on it.
                gap * 1.25 + 1.0
            } else {
                gap + 1.0
            }
        } else {
            0.0
        };
        // Every term here either ages out (gap window, burst detection) or is
        // recomputed from a bounded history (histogram). Nothing latches its own
        // peak — v4 had two terms that did (`peak_mode_height`, `cumsum_target`)
        // and they held the target at the comfort cap while this gap signal read
        // 10 frames. Do not add a term that cannot fall on its own.
        let target = (Self::min_depth_frames(config) as f32)
            .max(histogram_base)
            .max(burst_floor)
            .max(gap_floor);
        let cap = tcp_cap_override.unwrap_or(Self::comfort_cap_frames(config));
        let safe_cap = cap.max(Self::min_depth_frames(config) as f32);
        target
            .ceil()
            .clamp(Self::min_depth_frames(config) as f32, safe_cap) as u32
    }

    /// NetEQ `DelayManager::BufferLimits` — the drain decision band around the
    /// effective target, in whole frames. The orchestrator compares the filtered
    /// buffer level against these:
    ///   - `filtered >= 4 * high`  → emergency drain (fast accelerate, no cooldown)
    ///   - `filtered >= high`      → gentle drain (normal accelerate, cooldown)
    ///   - `filtered <  low`       → preemptive expand (slow down)
    ///
    /// `low = 0.75 * target`; `high = max(target, low + MIN_BAND)`. The minimum
    /// spread guarantees a dead-band between the expand and accelerate decisions so
    /// we don't ping-pong between them. NetEQ uses a 20ms window; we widen it to
    /// 40ms because on jittery links (2.4GHz) the filtered level micro-oscillates by
    /// 60-100ms, and a 20ms band let every small swing cross a limit and fire a
    /// splice per oscillation — the per-oscillation stutter. A 40ms dead-band keeps
    /// those micro-swings inside it; only genuine excursions cross. Mirrors
    /// `delay_manager.cc:358-375` (adapted from Q8 packets to whole frames).
    pub fn buffer_limits(target: u32) -> (u32, u32) {
        /// Minimum low→high spread in frames (40ms / MILLIS_PER_FRAME). Wider than
        /// NetEQ's 20ms to absorb 2.4GHz micro-oscillation without triggering drain.
        const MIN_BAND: u32 = 40 / super::consts::MILLIS_PER_FRAME;
        let low = target * 3 / 4;
        let high = target.max(low + MIN_BAND);
        (low, high)
    }

    /// Reset hysteresis + ramp + probe state for a new config. Returns the new
    /// effective target so the orchestrator can compute its flush target.
    pub fn reset_for_config(&mut self, config: &JitterConfig) -> u32 {
        self.effective_target = ms_to_frames_ceil(config.min_depth_ms).max(2);
        self.ramp_goal = self.effective_target;
        self.target_exit_count = 0;
        self.ramp_countdown = 0;
        self.probe_down_countdown = PROBE_DOWN_INTERVAL;
        self.probe_floor = 0;
        self.starvation_timestamps.clear();
        self.last_starvation = None;
        self.last_floor_decay = Instant::now();
        self.effective_target
    }

    /// Full reset on stream restart.
    /// Preserves `probe_floor` and `starvation_timestamps` — the network
    /// hasn't changed, only the stream died. Starting at the learned
    /// floor avoids the cold-start cascade (2→12→32→44→52 in <1s)
    /// visible in the 2.4GHz v2 field test.
    pub fn reset(&mut self) {
        self.effective_target = self.probe_floor.max(2);
        self.ramp_goal = self.effective_target;
        self.target_exit_count = 0;
        self.ramp_countdown = 0;
        self.probe_down_countdown = PROBE_DOWN_INTERVAL;
        // probe_floor: PRESERVED — empirical knowledge of starvation threshold.
        // starvation_timestamps: PRESERVED — gates probe aggression after restart.
    }

    /// Apply hysteresis, quantization, rate-limited ramping, and downward probing
    /// to the raw computed target. Returns the smoothed effective target.
    ///
    /// `raw_target` is the orchestrator's current `compute_target_depth` result
    /// (already tcp-capped / static-overridden). `min_depth` is the config floor.
    pub fn advance(
        &mut self,
        config: &JitterConfig,
        stats: &JitterStats,
        raw_target: u32,
        min_depth: u32,
    ) -> u32 {
        // Static and No Buffer modes bypass hysteresis entirely.
        if config.static_target_ms.is_some() {
            self.effective_target = raw_target;
            self.ramp_goal = raw_target;
            return raw_target;
        }

        let quantum = Self::adaptive_quantum(config);
        let hysteresis = Self::adaptive_hysteresis(config);
        let cap = Self::comfort_cap_frames(config) as u32;
        // Clamp AFTER quantization. `quantize_target` rounds to the nearest
        // multiple, which can land above the cap — `quantize_target(50, 4) == 52`
        // is why the 2.4GHz field log shows `effective_target=52, cap=50`.
        let quantized =
            Self::quantize_target(raw_target, quantum).clamp(min_depth, cap.max(min_depth));
        let diff = self.effective_target.abs_diff(quantized);

        // Relax the learned floor on a timer, independent of ramp direction.
        self.relax_probe_floor(quantum, min_depth);

        if diff <= hysteresis {
            // Inside the dead-zone: no change, reset dwell counter.
            self.target_exit_count = 0;
        } else {
            self.target_exit_count += 1;
            if self.target_exit_count >= Self::adaptive_dwell(config) {
                // Sustained deviation — commit to new ramp goal.
                tracing::debug!(
                    "[JitterMgr] Target transition: effective={}→ramp_goal={}, raw={}, ema_jitter={:.2}, max_gap={:.1}, floor={}, stability={:.2}",
                    self.effective_target,
                    quantized,
                    raw_target,
                    stats.ema_jitter,
                    stats.max_gap_frames(),
                    self.probe_floor,
                    stats.stability_ratio(),
                );
                self.ramp_goal = quantized.max(self.probe_floor);
                self.target_exit_count = 0;
            }
        }

        // Rate-limited ramp toward the goal, in BOTH directions.
        // UP: RAMP_UP_STEP frames per RAMP_INTERVAL_UP callbacks. Fast enough to
        //     beat real degradation, gradual enough that the decision band's
        //     `low_limit` stays within a frame or two of the real buffer level —
        //     the instant jump this replaces was the source of the click train.
        // DOWN: 1 frame per RAMP_INTERVAL — we have excess buffer, no urgency,
        //       and slow descent lets the network prove stability.
        if self.effective_target != self.ramp_goal {
            if self.effective_target < self.ramp_goal {
                if self.ramp_countdown == 0 {
                    self.effective_target =
                        self.effective_target.saturating_add(RAMP_UP_STEP).min(self.ramp_goal);
                    self.ramp_countdown = RAMP_INTERVAL_UP;
                }
            } else if self.ramp_countdown == 0 {
                self.effective_target -= 1;
                // Never ramp below the learned starvation floor — that level
                // caused starvation before and will again.
                if self.effective_target < self.probe_floor {
                    tracing::debug!(
                        "[JitterMgr] probe_floor holding: effective clamped {}→{}, ramp_goal={}, floor={}",
                        self.effective_target,
                        self.probe_floor,
                        self.ramp_goal,
                        self.probe_floor,
                    );
                    self.effective_target = self.probe_floor;
                }
                self.ramp_countdown = RAMP_INTERVAL;
            }
            self.ramp_countdown = self.ramp_countdown.saturating_sub(1);
        } else if stats.stability_ratio() > 0.2
            && self.effective_target > min_depth
            // Allow probing even during unstable regime if current stability
            // is locally high enough — the regime lock is a coarse heuristic
            // and the probe_floor prevents re-probing below safe levels.
            && !stats
                .unstable_regime_until()
                .is_some_and(|until| Instant::now() < until && stats.stability_ratio() < 0.5)
        {
            // Active downward probing: when the network has been calm for
            // a sustained period, nudge the target down to discover the
            // lowest stable depth. Speed scales with confidence.
            //
            // Burst clustering no longer *blocks* probing, it only slows it to
            // the gated interval. A blanket block meant the target could only
            // ever move up for as long as the screen was off — and since
            // `max_gap_frames()` now guards the depth honestly, the buffer no
            // longer needs a probe veto to stay safe through a DTIM cycle.
            let probe_interval = if self.recent_starvation_count() > 0 || stats.burst_detected() {
                PROBE_GATED_INTERVAL
            } else if stats.stability_ratio() > 0.8 {
                60 // High confidence: probe every ~300ms
            } else {
                120 // Normal: probe every ~600ms
            };
            self.probe_down_countdown = self.probe_down_countdown.saturating_sub(1);
            if self.probe_down_countdown == 0 {
                self.probe_down_countdown = probe_interval;
                let probe_goal =
                    Self::quantize_target(self.effective_target.saturating_sub(quantum), quantum)
                        .max(min_depth)
                        .max(self.probe_floor);
                if probe_goal < self.effective_target {
                    tracing::debug!(
                        "[JitterMgr] Probe down: effective={}→probe_goal={}, floor={}, stability={:.2}",
                        self.effective_target,
                        probe_goal,
                        self.probe_floor,
                        stats.stability_ratio(),
                    );
                    self.ramp_goal = probe_goal;
                }
            }
        }
        self.effective_target
    }

    /// Apply post-starvation probe floor update. Called by the orchestrator when
    /// the buffer recovers after a starvation event. Sets `probe_floor` so future
    /// probing won't immediately descend back to the level that just starved.
    ///
    /// Deliberately does NOT consult `compute_target_depth` any more. That ran
    /// ~140ms into recovery, when `iat_percentile_target` and
    /// `inter_burst_gap_frames` are both inflated **by the starvation itself** —
    /// a positive feedback loop that took the floor 5→20 and 23→40 in single
    /// events. The honest depth signal is `stats.max_gap_frames()`, read live
    /// through `compute_target_depth` on the normal path where it can also fall
    /// again.
    ///
    /// What is left is a pure safety net for starvations no arrival gap explains
    /// (decode stall, CPU scheduling): one quantum above whatever depth starved,
    /// ceilinged at `cap/2` so the floor ALONE can never pin the target at the
    /// comfort cap.
    pub fn apply_starvation_floor(&mut self, config: &JitterConfig, stats: &JitterStats) {
        let quantum = Self::adaptive_quantum(config);
        let cap = Self::comfort_cap_frames(config) as u32;
        self.probe_floor = self
            .probe_floor
            .max(self.effective_target.saturating_add(quantum))
            .min((cap / 2).max(1));
        tracing::info!(
            "[JitterMgr] Starvation floor set: probe_floor={}, effective_target={}, cap={}, max_gap={:.1}, ema_jitter={:.2}",
            self.probe_floor,
            self.effective_target,
            cap,
            stats.max_gap_frames(),
            stats.ema_jitter,
        );
    }

    /// Relax the learned floor by one quantum per [`FLOOR_DECAY_SECS`], once
    /// [`FLOOR_HOLD_SECS`] of starvation-free playback have elapsed.
    ///
    /// This is the descent guarantee. The previous decay path lived inside the
    /// dwell-completion branch and required the quantized target to be moving UP
    /// *and* zero recent starvation — but when the floor is what's holding the
    /// target high, the quantized target is *below* `effective_target`, so the
    /// branch was unreachable. The floor became a one-way ratchet, and once it
    /// reached the comfort cap the target was pinned there permanently.
    fn relax_probe_floor(&mut self, quantum: u32, min_depth: u32) {
        if self.probe_floor <= min_depth {
            return;
        }
        let now = Instant::now();
        if self
            .last_starvation
            .is_some_and(|t| now.duration_since(t).as_secs() < FLOOR_HOLD_SECS)
        {
            return;
        }
        if now.duration_since(self.last_floor_decay).as_secs() < FLOOR_DECAY_SECS {
            return;
        }
        self.last_floor_decay = now;
        let relaxed = self.probe_floor.saturating_sub(quantum).max(min_depth);
        tracing::debug!(
            "[JitterMgr] probe_floor relaxing: {}→{}",
            self.probe_floor,
            relaxed,
        );
        self.probe_floor = relaxed;
    }

    /// Immediately jump `effective_target` and `ramp_goal` to `probe_floor` after
    /// starvation recovery. Eliminates the window where the buffer is undersized
    /// while the ramp catches up — every callback in that window is a starvation
    /// risk. Safe to do instantly (unlike a normal ramp-up) because the buffer is
    /// already empty here: there is no audio in flight for a splice to click on.
    pub fn jump_to_floor(&mut self) {
        if self.probe_floor > self.effective_target {
            tracing::debug!(
                "[JitterMgr] Starvation recovery jump: effective={}→{}",
                self.effective_target,
                self.probe_floor,
            );
            self.effective_target = self.probe_floor;
            self.ramp_goal = self.probe_floor;
            self.target_exit_count = 0;
        }
    }

    /// Record a starvation event for rate-limiting probe aggressiveness.
    /// Evicts timestamps older than STARVATION_WINDOW_SECS.
    pub fn record_starvation(&mut self) {
        let now = Instant::now();
        self.starvation_timestamps.push_back(now);
        self.last_starvation = Some(now);
        // Evict expired entries.
        let cutoff = now - std::time::Duration::from_secs(STARVATION_WINDOW_SECS);
        while self
            .starvation_timestamps
            .front()
            .is_some_and(|&t| t < cutoff)
        {
            self.starvation_timestamps.pop_front();
        }
    }

    /// How many starvation events occurred within the tracking window.
    fn recent_starvation_count(&self) -> usize {
        let cutoff = Instant::now() - std::time::Duration::from_secs(STARVATION_WINDOW_SECS);
        self.starvation_timestamps
            .iter()
            .filter(|&&t| t >= cutoff)
            .count()
    }

    /// Test-only: rewind the floor-relax clocks and forget recent starvations, so
    /// a unit test can exercise the descent path without sleeping through
    /// [`FLOOR_HOLD_SECS`] / [`FLOOR_DECAY_SECS`] of real time. Simulated packet
    /// timelines advance `Instant`s we construct ourselves; these two timers read
    /// `Instant::now()` because in production they must track real playback time.
    #[cfg(test)]
    pub(super) fn rewind_floor_clock_for_test(&mut self) {
        let long_ago = Instant::now() - std::time::Duration::from_secs(3600);
        self.last_starvation = Some(long_ago);
        self.last_floor_decay = long_ago;
        self.starvation_timestamps.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// 2.4GHz-shaped profile: 30ms floor, 800ms cap.
    fn cfg() -> JitterConfig {
        JitterConfig {
            min_depth_ms: 30,
            comfort_cap_ms: 800,
            peak_decay_halflife_ms: 0,
            resume_threshold_pct: 0.5,
            static_target_ms: None,
        }
    }

    /// Regression for the dominant v3 field bug: `probe_floor` was a one-way
    /// ratchet that walked to the comfort cap and pinned `effective_target`
    /// there forever. Two independent guarantees now prevent that — a hard
    /// `cap/2` ceiling and a starvation-free decay timer — and this asserts both.
    #[test]
    fn probe_floor_is_ceilinged_at_half_cap_and_decays_to_min_depth() {
        let config = cfg();
        let stats = JitterStats::new();
        let cap = TargetController::comfort_cap_frames(&config) as u32; // 80
        let min_depth = TargetController::min_depth_frames(&config); // 3
        let mut control = TargetController::new();

        // Hammer the floor from a target already at the cap — the exact shape of
        // the field cascade. It must saturate at cap/2, never at cap.
        control.effective_target = cap;
        for _ in 0..50 {
            control.apply_starvation_floor(&config, &stats);
        }
        assert_eq!(
            control.probe_floor,
            cap / 2,
            "the learned floor alone must never be able to pin the target at the comfort cap",
        );

        // Now grant sustained starvation-free time and let the ramp run. The
        // floor must walk all the way back down to min_depth on its own.
        let long_ago = Instant::now() - Duration::from_secs(3600);
        control.last_starvation = Some(long_ago);
        control.last_floor_decay = long_ago;
        for _ in 0..200 {
            control.advance(&config, &stats, min_depth, min_depth);
            // Each relaxation is gated on FLOOR_DECAY_SECS of wall clock, which a
            // unit test can't wait out — rewind the clock instead so we exercise
            // the decay path rather than the timer.
            control.last_floor_decay = long_ago;
        }
        assert_eq!(
            control.probe_floor, min_depth,
            "a starvation-free link must be able to relax the floor all the way down",
        );
    }

    /// A fresh starvation must re-arm the hold: the floor may not decay while
    /// the link is still misbehaving.
    #[test]
    fn probe_floor_holds_while_starvations_are_recent() {
        let config = cfg();
        let stats = JitterStats::new();
        let min_depth = TargetController::min_depth_frames(&config);
        let mut control = TargetController::new();
        control.effective_target = 20;
        control.record_starvation();
        control.apply_starvation_floor(&config, &stats);
        let floor_after_starvation = control.probe_floor;
        assert!(floor_after_starvation > min_depth);

        // `last_floor_decay` is stale, but the starvation is fresh — the hold wins.
        control.last_floor_decay = Instant::now() - Duration::from_secs(3600);
        for _ in 0..100 {
            control.advance(&config, &stats, min_depth, min_depth);
        }
        assert_eq!(
            control.probe_floor, floor_after_starvation,
            "floor must not relax within FLOOR_HOLD_SECS of a starvation",
        );
    }

    /// The field log shows `effective_target=52, cap=50`: `quantize_target`
    /// rounds to the nearest multiple, which can land *above* the cap. The clamp
    /// now happens after quantization.
    #[test]
    fn effective_target_never_exceeds_the_comfort_cap() {
        // 500ms cap reproduces the exact logged case.
        let config = JitterConfig {
            comfort_cap_ms: 500,
            ..cfg()
        };
        let stats = JitterStats::new();
        let cap = TargetController::comfort_cap_frames(&config) as u32; // 50
        let min_depth = TargetController::min_depth_frames(&config);
        let mut control = TargetController::new();

        for _ in 0..5000 {
            // Raw target far above the cap, sustained — the worst case.
            let target = control.advance(&config, &stats, cap * 4, min_depth);
            assert!(
                target <= cap,
                "effective_target {target} exceeded comfort cap {cap}",
            );
        }
        assert!(
            control.effective_target >= cap - 4,
            "sustained over-cap demand should still pin the target near the cap, got {}",
            control.effective_target,
        );
    }

    /// Click-train fix #1. An instant 15-frame jump in `effective_target` drops
    /// `low_limit` far below the real buffer level, and every callback in that
    /// window used to fire a preemptive WSOLA expand — the "fast clicking on
    /// every buffer increase" from the field test. Growth is now rate-limited.
    #[test]
    fn upward_ramp_is_rate_limited() {
        let config = cfg();
        let stats = JitterStats::new();
        let min_depth = TargetController::min_depth_frames(&config);
        let mut control = TargetController::new();

        let mut prev = control.effective_target;
        let mut worst_step = 0;
        for _ in 0..4000 {
            let target = control.advance(&config, &stats, 60, min_depth);
            worst_step = worst_step.max(target.saturating_sub(prev));
            prev = target;
        }
        assert!(
            worst_step <= RAMP_UP_STEP,
            "single-callback target increase of {worst_step} frames exceeds RAMP_UP_STEP={RAMP_UP_STEP}",
        );
        assert!(
            control.effective_target >= 40,
            "the rate limit must not stop the climb, only pace it — got {}",
            control.effective_target,
        );
    }

    /// The gap tracker must be able to raise the target on its own, without a
    /// starvation to teach it — that is the whole point of the signal.
    #[test]
    fn observed_delivery_gap_raises_the_target_without_starving() {
        let config = cfg();
        let mut stats = JitterStats::new();
        let control = TargetController::new();
        let baseline = control.compute_target_depth(&config, &stats, None);

        // One 200ms DTIM-shaped gap.
        let t0 = Instant::now();
        stats.observe(1, t0);
        stats.observe(2, t0 + Duration::from_millis(210));

        let raised = control.compute_target_depth(&config, &stats, None);
        assert!(
            raised >= 21 && raised > baseline,
            "a 200ms delivery gap must lift the target to at least 21 frames (was {baseline}, got {raised})",
        );
    }

    /// The gap window in **isolation**, which is what makes it the load-bearing
    /// v4 signal. In the field a DTIM gap affects one arrival in 20-60, so it sits
    /// above the 95th percentile *of packets* — `iat_percentile_target` cannot see
    /// it, and `burst_detected` needs a full cluster pattern to fire. Populating
    /// only the gap window reproduces that blind spot exactly: every other term in
    /// the `.max()` chain reads zero, and the target must still cover the gap.
    #[test]
    fn the_gap_window_alone_is_enough_to_cover_a_dtim_gap() {
        let config = cfg();
        let mut stats = JitterStats::new();
        let control = TargetController::new();
        let min_depth = TargetController::min_depth_frames(&config);
        let baseline = control.compute_target_depth(&config, &stats, None);
        assert!(
            baseline <= min_depth + 2,
            "a virgin stats object must sit near the floor (min_depth={min_depth}), got {baseline}",
        );

        // A 500ms gap, recorded with nothing else touched.
        stats.record_gap(50.0, Instant::now());
        let raised = control.compute_target_depth(&config, &stats, None);
        assert!(
            raised >= 51,
            "the delivery-gap window alone must cover a 500ms gap (≥51 frames), got {raised}",
        );
        assert!(
            raised <= TargetController::comfort_cap_frames(&config) as u32,
            "still bounded by the comfort cap",
        );
    }

    /// The v4 field bug as a unit test. On 2.4GHz Router A the log recorded
    /// `effective_target=80` (the comfort cap) while `max_gap=9.7` — the only
    /// terms that could produce that were `max_iat_cumulative_sum` (a peak-latch
    /// of the running late-excess sum) and `peak_mode_height` (up to 8 latched
    /// peaks). Both re-measured the starvation-recovery burst that the gap window
    /// had already recorded once, then held that peak long after the window let
    /// go.
    ///
    /// Two guarantees, both asserted here:
    ///   1. **Bounded**: while the abuse is happening, the target may never
    ///      exceed what the honest gap signal justifies plus its headroom.
    ///   2. **Falls**: once the abuse stops, the target must collapse back to the
    ///      floor. This is the half a latch cannot satisfy — re-adding either one
    ///      leaves the target elevated for tens of seconds of clean link.
    #[test]
    fn no_statistic_may_latch_the_target_above_the_observed_gap() {
        let config = cfg();
        let mut stats = JitterStats::new();
        let control = TargetController::new();
        let min_depth = TargetController::min_depth_frames(&config);
        let quantum = TargetController::adaptive_quantum(&config);

        // Ten starve → recover → burst cycles: 361ms of silence, then the 40-frame
        // recovery burst delivered 1ms apart. Real-time honest (401ms of wall clock
        // for 41 frames of audio) so nothing drifts; the only real signal is a
        // repeating ~36-frame delivery gap.
        let mut t = Instant::now();
        let mut seq = 1u64;
        stats.observe(seq, t);
        for _ in 0..10 {
            t += Duration::from_millis(361);
            seq += 1;
            stats.observe(seq, t);
            for _ in 0..40 {
                t += Duration::from_millis(1);
                seq += 1;
                stats.observe(seq, t);
                let target = control.compute_target_depth(&config, &stats, None);
                let bound = (min_depth as f32)
                    .max(stats.max_gap_frames() * 1.25 + quantum as f32 + 1.0)
                    .ceil() as u32;
                assert!(
                    target <= bound,
                    "target {target} exceeds what max_gap={:.1} can justify (bound {bound}) — \
                     something in the .max() chain is latching its own history",
                    stats.max_gap_frames(),
                );
            }
        }

        // Abuse over. 22s of a perfect link must return the target to the floor.
        for _ in 0..2200 {
            t += Duration::from_millis(10);
            seq += 1;
            stats.observe(seq, t);
        }
        let settled = control.compute_target_depth(&config, &stats, None);
        assert!(
            settled <= min_depth + quantum,
            "after 22s of a clean link the target must be back at the floor \
             (min_depth={min_depth}), got {settled}",
        );
    }
}
