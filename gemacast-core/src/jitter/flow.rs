//! Playback-lifecycle actor: owns the state machine that tracks whether we are
//! prebuffering, starving, or holding for a reordered packet, plus the NetEQ
//! IIR-filtered buffer level. Owns no jitter buffer or decoder — the orchestrator
//! reads these counters to sequence prebuffer / gap / play / starve transitions.

/// Rolling playback-lifecycle state derived from buffer occupancy over time.
pub(super) struct PlaybackFlow {
    /// True while accumulating the initial buffer before playback starts.
    pub is_prebuffering: bool,
    /// Consecutive callbacks with no playable frame (drives the hard-reset timeout).
    pub missing_count: u32,
    /// Consecutive callbacks the buffer has been fully empty (drives rebuffer).
    pub starvation_count: u32,
    /// How many consecutive callbacks we've been waiting for the current gap slot.
    /// Prevents spurious PLC for late-arriving reordered packets on 2.4GHz.
    pub gap_hold_count: u32,
    /// NetEQ-style starvation recovery guard. After the buffer drains to
    /// near-zero (starvation), suppress ALL acceleration for this many
    /// callbacks to let the buffer refill. Prevents the drain→starve→
    /// refill→drain saw-tooth cycle. Matches `prev_mode != kModeExpand`
    /// guard in WebRTC's decision_logic.cc:278.
    pub starvation_recovery: u32,
    /// NetEQ-style IIR filtered buffer level to ignore instantaneous OS batching spikes.
    pub filtered_buffer_level: f32,
}

impl PlaybackFlow {
    pub fn new() -> Self {
        Self {
            is_prebuffering: true,
            missing_count: 0,
            starvation_count: 0,
            gap_hold_count: 0,
            starvation_recovery: 0,
            filtered_buffer_level: 0.0,
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

    /// Per-callback tick of the starvation-recovery guard.
    pub fn tick_recovery(&mut self) {
        self.starvation_recovery = self.starvation_recovery.saturating_sub(1);
    }

    /// Partial reset on stream restart (matches the legacy `trigger_reset` field set):
    /// re-enters prebuffering and zeroes the missing/starvation/gap counters.
    ///
    /// `filtered_buffer_level` is snapped to zero rather than left to coast: the IIR
    /// time constant is ~1.3s at a large target, so a stale pre-restart reading
    /// survives well past the recovery guard and misdirects the very first
    /// drain/expand decision after the stream comes back. `starvation_recovery` is
    /// deliberately left untouched — a restart is not a reason to re-enable drain.
    pub fn reset_on_stream_restart(&mut self) {
        self.is_prebuffering = true;
        self.missing_count = 0;
        self.starvation_count = 0;
        self.gap_hold_count = 0;
        self.filtered_buffer_level = 0.0;
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
