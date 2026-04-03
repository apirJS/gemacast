use std::collections::VecDeque;
use std::time::Instant;

use super::types::DspCommand;

/// Maximum trackable inter-arrival delay in milliseconds.
const MAX_DELAY_MS: usize = 200;

/// Number of recent packets used for the sliding-window histogram.
const HISTORY_WINDOW: usize = 100;

/// Minimum target buffer depth in frames (floor to prevent underruns).
const MIN_TARGET_FRAMES: u32 = 2; // 20ms

/// Maximum target buffer depth in frames (ceiling to cap latency).
const MAX_TARGET_FRAMES: u32 = 20; // 200ms

/// Number of consecutive missing frames before forcing re-prebuffer.
const MAX_CONSECUTIVE_MISSING: u32 = 10; // 100ms of silence → rebuffer

/// The "brain" of the jitter buffer system.
///
/// Combines two algorithms:
/// 1. **Sliding-window histogram** of inter-arrival jitter → computes the 95th-percentile
///    target buffer depth (how deep the buffer *should* be to absorb Wi-Fi variance).
/// 2. **PI controller** that smoothly adjusts a WSOLA stretch factor to converge
///    actual buffer depth toward the target without audible artifacts.
pub struct JitterController {
    // --- Histogram state ---
    /// Bucket `i` counts how many of the last `HISTORY_WINDOW` packets had `i` ms of jitter.
    delay_buckets: [u32; MAX_DELAY_MS],
    /// FIFO of recent delay values so we can expire old entries from the histogram.
    delay_history: VecDeque<usize>,
    /// Total packets currently in the histogram (≤ HISTORY_WINDOW).
    total_packets: u32,

    // --- Arrival tracking ---
    /// Timestamp of the last received packet (for computing inter-arrival deltas).
    last_arrival: Option<Instant>,
    /// Sequence number of the last received packet.
    last_seq_num: Option<u64>,

    // --- Concealment tracking ---
    /// How many consecutive frames were missing (for rebuffer detection).
    consecutive_missing: u32,

    // --- Prebuffering ---
    /// When true, we output silence until we accumulate enough depth.
    prebuffering: bool,
    /// How many frames we require before starting playback.
    prebuffer_target: u32,
}

impl JitterController {
    pub fn new() -> Self {
        Self {
            delay_buckets: [0; MAX_DELAY_MS],
            delay_history: VecDeque::with_capacity(HISTORY_WINDOW),
            total_packets: 0,
            last_arrival: None,
            last_seq_num: None,

            consecutive_missing: 0,
            prebuffering: true,
            prebuffer_target: 4, // 40ms initial prebuffer
        }
    }

    /// Record a packet's arrival for the jitter histogram.
    ///
    /// Computes the absolute inter-arrival jitter: the difference between
    /// the actual inter-arrival time and the expected time based on sequence gaps.
    pub fn record_arrival(&mut self, seq_num: u64, arrival_time: Instant) {
        if let (Some(last_time), Some(last_seq)) = (self.last_arrival, self.last_seq_num)
            && seq_num > last_seq
        {
            let elapsed_ms = arrival_time.duration_since(last_time).as_millis() as usize;
            // Each sequence gap represents 10ms of expected audio.
            let expected_ms = ((seq_num - last_seq) as usize) * 10;
            let jitter_ms = elapsed_ms.abs_diff(expected_ms);
            self.add_delay(jitter_ms);
        }
        self.last_arrival = Some(arrival_time);
        self.last_seq_num = Some(seq_num);
    }

    /// Add a jitter measurement to the sliding-window histogram.
    fn add_delay(&mut self, delay_ms: usize) {
        let clamped = delay_ms.min(MAX_DELAY_MS - 1);
        self.delay_buckets[clamped] += 1;
        self.delay_history.push_back(clamped);
        self.total_packets += 1;

        // Expire the oldest entry if we exceed the window size.
        if self.delay_history.len() > HISTORY_WINDOW
            && let Some(old) = self.delay_history.pop_front()
        {
            self.delay_buckets[old] = self.delay_buckets[old].saturating_sub(1);
            self.total_packets = self.total_packets.saturating_sub(1);
        }
    }

    /// Whether we're still accumulating initial buffer depth.
    pub fn is_prebuffering(&self) -> bool {
        self.prebuffering
    }

    /// Check if we've accumulated enough depth to exit prebuffering.
    pub fn check_prebuffer(&mut self, depth: u32) {
        if self.prebuffering && depth >= self.prebuffer_target {
            self.prebuffering = false;
        }
    }

    /// Enter prebuffering mode (e.g., after sustained packet loss).
    pub fn enter_prebuffering(&mut self) {
        self.prebuffering = true;
    }

    /// Decide what DSP command to issue for the current frame.
    ///
    /// # Arguments
    /// - `current_depth_frames`: How many contiguous frames the jitter buffer has ready.
    /// - `has_next_frame`: Whether the next expected sequence number is available.
    pub fn decide(&mut self, current_depth_frames: u32, has_next_frame: bool) -> DspCommand {
        if !has_next_frame {
            self.consecutive_missing += 1;

            // After too many consecutive losses, rebuffer to resync.
            if self.consecutive_missing >= MAX_CONSECUTIVE_MISSING {
                self.enter_prebuffering();
            }

            return DspCommand::Conceal;
        }

        self.consecutive_missing = 0;

        // Calculate the ideal buffer depth from the jitter histogram.
        let target_depth = self.target_depth_frames();

        // Instead of a hyper-active PI controller triggering continuous granular WSOLA (which causes
        // massive phase discontinuities at 10ms frame boundaries), we only intervene if the
        // buffer depth has drifted completely out of acceptable bounds.

        // If we've built up way too much buffering (e.g., 100ms over target), we drop a frame to catch up.
        // This only happens rarely during major network latency recoveries or slow clock skew.
        if current_depth_frames > target_depth + 10 {
            DspCommand::Accelerate { factor: 1.0 }
        }
        // If we're dangerously close to underruning (e.g., 40ms under target), we generate a pristine
        // PLC frame to expand the buffer and buy time for late packets to arrive.
        else if current_depth_frames < target_depth.saturating_sub(4) && current_depth_frames > 0
        {
            DspCommand::Expand { factor: 1.0 }
        }
        // Otherwise, do nothing! Just play the frames continuously.
        else {
            DspCommand::Normal
        }
    }

    /// Compute the target buffer depth in frames from the 95th-percentile jitter.
    fn target_depth_frames(&self) -> u32 {
        let p95_ms = self.percentile_95();
        // Convert ms → frames (1 frame = 10ms), with clamped floor/ceiling.
        ((p95_ms / 10) + 1).clamp(MIN_TARGET_FRAMES, MAX_TARGET_FRAMES)
    }

    /// Compute the 95th percentile of the jitter histogram in milliseconds.
    fn percentile_95(&self) -> u32 {
        if self.total_packets == 0 {
            return 30; // Default 30ms when no data yet.
        }
        let threshold = (self.total_packets as f32 * 0.95) as u32;
        let mut cumulative = 0u32;
        for (ms, &count) in self.delay_buckets.iter().enumerate() {
            cumulative += count;
            if cumulative >= threshold {
                return ms as u32;
            }
        }
        30
    }

    /// Reset all state. Called on disconnect/reconnect.
    pub fn reset(&mut self) {
        self.delay_buckets = [0; MAX_DELAY_MS];
        self.delay_history.clear();
        self.total_packets = 0;
        self.last_arrival = None;
        self.last_seq_num = None;

        self.consecutive_missing = 0;
        self.prebuffering = true;
    }
}
