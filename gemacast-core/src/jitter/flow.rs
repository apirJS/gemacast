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
    /// Alpha = 254/256 ≈ 0.9921875. Heavily low-passes the instantaneous buffer
    /// level so that massive batching (e.g. 10 packets arriving at once via USB)
    /// doesn't trigger an instantaneous flush. Updates and returns the new level.
    pub fn filter_buffer_level(&mut self, occupied: u32) -> f32 {
        let alpha = 254.0 / 256.0;
        self.filtered_buffer_level =
            self.filtered_buffer_level * alpha + (occupied as f32) * (1.0 - alpha);
        self.filtered_buffer_level
    }

    /// Per-callback tick of the starvation-recovery guard.
    pub fn tick_recovery(&mut self) {
        self.starvation_recovery = self.starvation_recovery.saturating_sub(1);
    }

    /// Partial reset on stream restart (matches the legacy `trigger_reset` field set):
    /// re-enters prebuffering and zeroes the missing/starvation/gap counters.
    /// Deliberately leaves `filtered_buffer_level` and `starvation_recovery` untouched.
    pub fn reset_on_stream_restart(&mut self) {
        self.is_prebuffering = true;
        self.missing_count = 0;
        self.starvation_count = 0;
        self.gap_hold_count = 0;
    }
}
