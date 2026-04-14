use std::collections::VecDeque;

use opus::Decoder;
use ringbuf::{HeapCons, traits::*};

use crate::audio::{OPUS_CHANNELS, OPUS_FRAME_SAMPLES};

use super::buffer::JitterBuffer;
use super::types::RawPacket;

/// 2 frames × 10ms = 20ms absolute floor.
/// The adaptive `prebuffer_target` provides the real safety net above this.
const MIN_DEPTH: u32 = 2;
const MAX_MISSING: u32 = 200;

/// Reorder tolerance: process_next_frame callbacks to wait for a reordered packet.
/// At 250 callbacks/sec: 3 = ~12ms window. Short enough to not stall on genuine loss.
const REORDER_TOLERANCE: u32 = 3;

/// After this many consecutive clean-play callbacks, use the fast EMA decay rate.
/// 500 callbacks ≈ 2 seconds. Allows the prebuffer target to shrink back to MIN_DEPTH
/// much faster during stable network periods.
const CLEAN_DECAY_THRESHOLD: u32 = 500;

/// Slow EMA decay: used after any recent starvation or jitter event.
/// 0.998^250 ≈ 60% per second.
const EMA_DECAY_SLOW: f32 = 0.998;

/// Fast EMA decay: used when the buffer has been playing cleanly for 2+ seconds.
/// 0.992^250 ≈ 14% per second. Squeezes latency back to floor 4× faster.
const EMA_DECAY_FAST: f32 = 0.992;

/// Hard flush ceiling — frames above the current prebuffer target before Tier 2 fires.
/// 20 frames = 200ms of extra headroom. The buffer is never touched below this line.
/// Only catastrophic clock drift or prolonged freeze will trigger a flush.
const MAX_HEADROOM: u32 = 20;

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Instant;

/// Coordinates the full jitter buffer pipeline.
///
/// Owns all components (buffer, Opus decoder) and exposes
/// a simple two-method API:
///
/// 1. `ingest_packets()` — drain raw packets from the SPSC channel into the ordered buffer.
/// 2. `fill_output()` — produce exactly the requested number of PCM samples for the cpal callback.
///
/// # Thread model
///
/// This entire struct lives inside the cpal output callback closure.
/// It is **never** shared across threads. The network thread communicates
/// via the lock-free `HeapCons<RawPacket>` ring buffer.
pub struct JitterBufferManager {
    decoder: Decoder,
    buffer: JitterBuffer,
    /// Accumulator of processed PCM samples ready for cpal to consume.
    /// Decouples the Opus frame size (960 samples) from cpal's variable buffer size.
    playback_buf: VecDeque<f32>,
    /// Reusable buffer for Opus decode output (avoids per-frame allocation).
    /// IMPORTANT: Always kept at full capacity (OPUS_FRAME_SAMPLES) — never truncated.
    decode_buf: Vec<f32>,
    /// How many valid samples are in decode_buf after the last decode.
    decode_len: usize,

    is_prebuffering: bool,
    missing_count: u32,
    starvation_count: u32,

    /// The frame depth the buffer must reach before exiting prebuffering.
    /// Set by `compute_prebuffer_target()` each time starvation triggers a rebuffer.
    /// Predictive: it adapts to the learned network burst size so we never exit
    /// prebuffering into a network that will immediately re-starve.
    prebuffer_target: u32,

    /// EMA of network variance (frames). Drives `compute_prebuffer_target()`.
    ema_jitter_ms: f32,
    last_arrival: Option<Instant>,
    last_seq: Option<u64>,

    /// Consecutive callbacks where `has_next()` was true and a frame decoded cleanly.
    /// Used to switch from slow to fast EMA decay, accelerating latency recovery.
    clean_play_count: u32,

    /// Stamping point for true NIC->DAC millisecond latency. Shared with receiver backend.
    latency_metric: Arc<AtomicU32>,
    /// How many consecutive callbacks we've been waiting for the current gap slot.
    /// Prevents spurious PLC for late-arriving reordered packets on 2.4GHz.
    gap_hold_count: u32,

    /// Counter for active latency shaving (Soft Trim).
    frames_since_last_trim: u32,
}

impl JitterBufferManager {
    pub fn new(decoder: Decoder, latency_metric: Arc<AtomicU32>) -> Self {
        Self {
            decoder,
            buffer: JitterBuffer::new(),
            playback_buf: VecDeque::with_capacity(OPUS_FRAME_SAMPLES * 4),
            decode_buf: vec![0.0f32; OPUS_FRAME_SAMPLES],
            decode_len: 0,
            is_prebuffering: true,
            missing_count: 0,
            starvation_count: 0,
            prebuffer_target: MIN_DEPTH,
            ema_jitter_ms: 0.0,
            last_arrival: None,
            last_seq: None,
            clean_play_count: 0,
            latency_metric,
            gap_hold_count: 0,
            frames_since_last_trim: 0,
        }
    }

    /// Drain all pending raw packets from the SPSC channel into the jitter buffer.
    ///
    /// Called at the top of every cpal callback. The consumer is the audio-thread
    /// side of the lock-free ring buffer shared with the network thread.
    pub fn ingest_packets(&mut self, consumer: &mut HeapCons<RawPacket>) {
        let mut burst_size = 0;
        while let Some(pkt) = consumer.try_pop() {
            if let (Some(last_arr), Some(last_s)) = (self.last_arrival, self.last_seq) {
                if pkt.seq_num > last_s && (pkt.seq_num - last_s) < 1000 {
                    let elapsed = pkt.arrival_time.duration_since(last_arr).as_millis() as f32;
                    let seq_delta = (pkt.seq_num - last_s) as f32;
                    let expected = seq_delta * 10.0;

                    let variance_ms = (elapsed - expected).abs();
                    let variance_frames = variance_ms / 10.0;
                    if variance_frames > self.ema_jitter_ms {
                        self.ema_jitter_ms = variance_frames; // Spike on arrival jitter
                    }
                }
            }
            self.last_arrival = Some(pkt.arrival_time);
            self.last_seq = Some(pkt.seq_num);
            self.buffer.insert(pkt);
            burst_size += 1;
        }

        if burst_size > 0 {
            let burst_f32 = burst_size as f32;
            if burst_f32 > self.ema_jitter_ms {
                self.ema_jitter_ms = burst_f32; // Spike on burst aggregation
            }
        }

        // Adaptive decay: fast when stable, slow when recovering from jitter.
        // fast = 0.992^250/s ≈ 14% left per second → returns 15-frame spike to floor in ~1s
        // slow = 0.998^250/s ≈ 60% left per second → returns 15-frame spike to floor in ~4s
        let decay = if self.clean_play_count >= CLEAN_DECAY_THRESHOLD {
            EMA_DECAY_FAST
        } else {
            EMA_DECAY_SLOW
        };

        // Stop decaying target during starvation! (Gap physics)
        // If we are actively starved, we maintain the highest required peak.
        if self.buffer.occupied_count() > 0 || self.clean_play_count > 0 {
            self.ema_jitter_ms *= decay;
        }
    }

    /// Compute the playout depth the buffer must reach before exiting prebuffering.
    ///
    /// This is the predictive element of the AJB: before playback restarts, we wait
    /// until we have enough frames to survive the next expected burst gap. If the
    /// network has been delivering 15-frame bursts, we wait for 15 frames so the
    /// first burst gap doesn't immediately re-starve us.
    fn compute_prebuffer_target(&self) -> u32 {
        (self.ema_jitter_ms as u32).clamp(MIN_DEPTH, 300)
    }

    /// Fill `output` with PCM samples.
    ///
    /// No active shedding. Latency control is handled exclusively by:
    ///   - Tier 2: hard flush in `process_next_frame` if backlog is catastrophic.
    ///   - Predictive prebuffering: starvation triggers a re-anchor at the correct depth.
    pub fn fill_output(&mut self, output: &mut [f32], volume: f32) {
        for sample in output.iter_mut() {
            if self.playback_buf.is_empty() {
                self.process_next_frame();
            }
            *sample = self.playback_buf.pop_front().unwrap_or(0.0) * volume;
        }
    }

    /// Process one Opus frame from the jitter buffer into the playback buffer.
    ///
    /// INVARIANT: always appends exactly OPUS_FRAME_SAMPLES to playback_buf.
    ///
    /// Control flow (in priority order):
    ///  1. Prebuffering: output silence until MIN_DEPTH packets are ready
    ///  2. `has_next()` happy path: pop and decode the expected packet
    ///  3. `occupied == 0`: true starvation — PLC, then rebuffer after 50 frames
    ///  4. Gap: future packets exist but the current slot is empty
    ///     - Large gap (>20 frames): immediate fast_forward to playable audio
    ///     - Small gap (≤20 frames): wait REORDER_TOLERANCE callbacks (80ms)
    ///       before declaring the slot permanently lost via advance_one()
    ///
    /// NOTE: pop_next() is ONLY called on the happy path where has_next()==true.
    /// This is critical: in the gap path, next_play_seq must NOT be advanced by
    /// pop_next, or late-arriving reordered packets become stale and are dropped.
    fn process_next_frame(&mut self) {
        // Tier 2: catastrophic hard flush. Only fires when the buffer has grown
        // far beyond the current prebuffer target (e.g. after a prolonged freeze).
        // Between target and target+200ms the buffer is left completely untouched.
        if self.buffer.occupied_count() > self.prebuffer_target + MAX_HEADROOM {
            while self.buffer.occupied_count() > self.prebuffer_target {
                self.buffer.advance_one();
            }
            let _ = self.decoder.reset_state();
        }

        if self.is_prebuffering {
            // Exit prebuffering only when we have `prebuffer_target` frames.
            // This target was set by compute_prebuffer_target() at the last starvation event,
            // so it reflects the network's actual burst size — we won't start playback
            // into a network that will immediately re-starve us.
            if self.buffer.occupied_count() >= self.prebuffer_target {
                self.is_prebuffering = false;
            } else {
                self.playback_buf
                    .extend(std::iter::repeat_n(0.0, OPUS_FRAME_SAMPLES));
                return;
            }
        }

        if self.buffer.has_next() {
            self.gap_hold_count = 0;
            self.missing_count = 0;
            self.starvation_count = 0;
            self.clean_play_count = self.clean_play_count.saturating_add(1);

            // Soft Trimming (Active Latency Shaving)
            // If physical depth > target depth + 2, gently drop 1 frame every 50 frames (500ms)
            if self.buffer.occupied_count() > self.prebuffer_target + 2 && !self.is_prebuffering {
                self.frames_since_last_trim += 1;
                if self.frames_since_last_trim >= 50 {
                    self.buffer.advance_one(); // Discard the currently expected frame
                    self.frames_since_last_trim = 0;

                    // We just discarded a frame, so we must process the *next* frame.
                    if !self.buffer.has_next() {
                        self.generate_plc();
                        return;
                    }
                }
            } else {
                self.frames_since_last_trim = 0;
            }

            let pkt = self.buffer.pop_next().expect("has_next was true");
            let delay_ms = Instant::now().duration_since(pkt.arrival_time).as_millis() as u32;
            self.latency_metric.store(delay_ms, Ordering::Relaxed);
            if pkt.is_uncompressed {
                self.buffer_uncompressed(&pkt.payload_data);
            } else {
                self.decode_and_buffer(&pkt.payload_data);
            }
            return;
        }

        if self.buffer.occupied_count() == 0 {
            self.gap_hold_count = 0;
            self.missing_count += 1;
            self.starvation_count += 1;
            self.clean_play_count = 0; // Reset clean streak on starvation

            if self.missing_count > MAX_MISSING {
                self.trigger_reset();
                self.playback_buf
                    .extend(std::iter::repeat_n(0.0, OPUS_FRAME_SAMPLES));
                return;
            }

            // Re-anchor after 5 consecutive starved frames (50ms).
            // Recompute the prebuffer target NOW so we re-enter prebuffering
            // with the correct depth for the current network conditions.
            if self.starvation_count >= 5 {
                self.prebuffer_target = self.compute_prebuffer_target();
                self.is_prebuffering = true;
                self.starvation_count = 0;
                self.gap_hold_count = 0;
                let _ = self.decoder.reset_state();
                self.playback_buf
                    .extend(std::iter::repeat_n(0.0, OPUS_FRAME_SAMPLES));
                return;
            }

            self.generate_plc();
            return;
        }

        self.starvation_count = 0;
        self.clean_play_count = 0; // Reset clean streak on gap

        let gap_size = self
            .buffer
            .lowest_available_seq()
            .map(|lo| lo.saturating_sub(self.buffer.next_play_seq()))
            .unwrap_or(0);

        if gap_size > 20 {
            if let Some(lowest) = self.buffer.lowest_available_seq() {
                self.buffer.fast_forward(lowest);
                let _ = self.decoder.reset_state();
            }
            self.missing_count += 1;
            self.gap_hold_count = 0;
        } else {
            self.gap_hold_count += 1;
            if self.gap_hold_count >= REORDER_TOLERANCE {
                if let Some(lowest) = self.buffer.lowest_available_seq() {
                    self.buffer.fast_forward(lowest);
                } else {
                    self.buffer.advance_one();
                }
                self.missing_count += 1;
                self.gap_hold_count = 0;
            }
        }

        if self.missing_count > MAX_MISSING {
            self.trigger_reset();
            self.playback_buf
                .extend(std::iter::repeat_n(0.0, OPUS_FRAME_SAMPLES));
            return;
        }

        self.generate_plc();
    }

    fn trigger_reset(&mut self) {
        self.buffer.reset();
        self.is_prebuffering = true;
        self.prebuffer_target = MIN_DEPTH;
        self.missing_count = 0;
        self.starvation_count = 0;
        self.gap_hold_count = 0;
        self.ema_jitter_ms = 0.0;
        self.last_arrival = None;
        self.last_seq = None;
        self.clean_play_count = 0;
        self.frames_since_last_trim = 0;
        self.playback_buf.clear();
        self.decode_buf.fill(0.0);
        self.decode_len = 0;
        let _ = self.decoder.reset_state();
    }

    /// Decode an Opus packet and append the PCM to the playback buffer.
    fn decode_and_buffer(&mut self, opus_data: &[u8]) {
        if self.decode_opus(opus_data) {
            self.playback_buf
                .extend(&self.decode_buf[..self.decode_len]);
        } else {
            self.generate_plc();
        }
    }

    /// Directly append raw uncompressed PCM data into the playback buffer.
    fn buffer_uncompressed(&mut self, pcm_bytes: &[u8]) {
        let f32_len = pcm_bytes.len() / std::mem::size_of::<f32>();
        if f32_len == 0 {
            self.generate_plc();
            return;
        }

        for chunk in pcm_bytes.chunks_exact(4) {
            let f = f32::from_ne_bytes(chunk.try_into().unwrap());
            self.playback_buf.push_back(f);
        }

        let _ = self.decoder.decode_float(&[], &mut self.decode_buf, false);
    }

    /// Decode an Opus packet into `self.decode_buf`.
    /// Returns `true` on success.
    ///
    /// IMPORTANT: We track the valid length separately in `self.decode_len`
    /// instead of truncating the Vec. Truncating would shrink the slice
    /// passed to subsequent `decode_float` calls, potentially causing failures
    /// if a prior decode returned fewer samples than expected.
    fn decode_opus(&mut self, opus_data: &[u8]) -> bool {
        match self
            .decoder
            .decode_float(opus_data, &mut self.decode_buf, false)
        {
            Ok(samples_per_channel) => {
                self.decode_len = samples_per_channel * OPUS_CHANNELS as usize;
                true
            }
            Err(_) => false,
        }
    }

    /// Use Opus's native PLC to hallucinate a missing frame.
    ///
    /// This leverages the decoder's internal state from the last good packet
    /// to synthesize plausible audio. Much better than silence — it preserves
    /// the spectral envelope and pitch.
    fn generate_plc(&mut self) {
        match self
            .decoder
            .decode_float(&[] as &[u8], &mut self.decode_buf, false)
        {
            Ok(samples_per_channel) => {
                let total = samples_per_channel * OPUS_CHANNELS as usize;
                self.playback_buf.extend(&self.decode_buf[..total]);
            }
            Err(_) => {
                // Last resort: silence.
                self.playback_buf
                    .extend(std::iter::repeat_n(0.0, OPUS_FRAME_SAMPLES));
            }
        }
    }

    /// Reset all state. Called on disconnect/reconnect.
    pub fn reset(&mut self) {
        self.trigger_reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::{OPUS_FRAME_SAMPLES, OPUS_SAMPLE_RATE};
    use opus::{Application, Channels, Decoder, Encoder};
    use ringbuf::HeapRb;
    use std::time::Instant;

    fn setup_env() -> (
        JitterBufferManager,
        Encoder,
        ringbuf::HeapProd<RawPacket>,
        ringbuf::HeapCons<RawPacket>,
    ) {
        let decoder = Decoder::new(OPUS_SAMPLE_RATE, Channels::Stereo).unwrap();
        let encoder = Encoder::new(OPUS_SAMPLE_RATE, Channels::Stereo, Application::Audio).unwrap();
        let atomic = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let manager = JitterBufferManager::new(decoder, atomic);
        let rb = HeapRb::<RawPacket>::new(1000);
        let (prod, cons) = rb.split();
        (manager, encoder, prod, cons)
    }

    fn make_packet(encoder: &mut Encoder, seq: u64, base_time: Instant) -> RawPacket {
        let pcm = vec![0.0f32; OPUS_FRAME_SAMPLES];
        let opus_data = encoder.encode_vec_float(&pcm, 1500).unwrap();
        RawPacket {
            seq_num: seq,
            payload_data: opus_data,
            arrival_time: base_time + std::time::Duration::from_millis(seq * 20),
            is_uncompressed: false,
        }
    }

    #[test]
    fn test_prebuffering_outputs_silence_until_target_depth() {
        let (mut manager, mut encoder, mut prod, mut cons) = setup_env();
        let base_time = Instant::now();

        // Push MIN_DEPTH - 1 packets: should still be prebuffering.
        for i in 1..MIN_DEPTH {
            assert!(
                prod.try_push(make_packet(&mut encoder, i as u64, base_time))
                    .is_ok()
            );
        }
        manager.ingest_packets(&mut cons);

        let mut output = vec![1.0; OPUS_FRAME_SAMPLES];
        manager.fill_output(&mut output, 1.0);

        for &sample in &output {
            assert_eq!(sample, 0.0, "Expected silence while prebuffering");
        }
        assert!(manager.is_prebuffering);

        // Push the final packet to reach MIN_DEPTH: should exit prebuffering.
        assert!(
            prod.try_push(make_packet(&mut encoder, MIN_DEPTH as u64, base_time))
                .is_ok()
        );
        manager.ingest_packets(&mut cons);

        manager.fill_output(&mut output, 1.0);
        assert!(!manager.is_prebuffering);
    }

    #[test]
    fn test_packet_loss_triggers_plc() {
        let (mut manager, mut encoder, mut prod, mut cons) = setup_env();
        let base_time = Instant::now();

        // Fill to exactly MIN_DEPTH to exit prebuffering.
        for i in 1..=MIN_DEPTH {
            assert!(
                prod.try_push(make_packet(&mut encoder, i as u64, base_time))
                    .is_ok()
            );
        }
        manager.ingest_packets(&mut cons);

        let mut output = vec![0.0; OPUS_FRAME_SAMPLES];
        manager.fill_output(&mut output, 1.0);
        assert!(!manager.is_prebuffering);

        // Push a packet with a gap (skip one seq num) to simulate packet loss.
        let gap_seq = (MIN_DEPTH + 2) as u64;
        assert!(
            prod.try_push(make_packet(&mut encoder, gap_seq, base_time))
                .is_ok()
        );
        manager.ingest_packets(&mut cons);

        // Drain the remaining valid packets.
        for _ in 2..=MIN_DEPTH {
            manager.fill_output(&mut output, 1.0);
        }

        // The missing packet in the gap triggers PLC.
        manager.fill_output(&mut output, 1.0);
        // With small gap (1 slot, <=20): waits REORDER_TOLERANCE callbacks before advancing.
        // After REORDER_TOLERANCE-1 waits the slot is declared lost (missing_count=1).
        // After 1 more call, the future packet (gap_seq) becomes the expected seq and plays.
        for _ in 0..(REORDER_TOLERANCE - 1) {
            manager.fill_output(&mut output, 1.0);
        }
        assert_eq!(manager.missing_count, 1);
        assert!(!manager.is_prebuffering);
    }

    #[test]
    fn test_starvation_triggers_rebuffering() {
        let (mut manager, mut encoder, mut prod, mut cons) = setup_env();
        let base_time = Instant::now();

        // Fill enough to exit prebuffering.
        for i in 1..=MIN_DEPTH {
            assert!(
                prod.try_push(make_packet(&mut encoder, i as u64, base_time))
                    .is_ok()
            );
        }
        manager.ingest_packets(&mut cons);

        let mut output = vec![0.0; OPUS_FRAME_SAMPLES];
        for _ in 1..=MIN_DEPTH {
            manager.fill_output(&mut output, 1.0);
        }
        assert!(!manager.is_prebuffering);

        // Frame 1 empty -> PLC
        manager.fill_output(&mut output, 1.0);
        assert_eq!(manager.starvation_count, 1);
        assert!(!manager.is_prebuffering);

        // Frame 2 empty -> PLC
        manager.fill_output(&mut output, 1.0);
        assert_eq!(manager.starvation_count, 2);
        assert!(!manager.is_prebuffering);

        // Drain exactly 50 starvation frames to hit the >= 50 threshold.
        for _ in 3..=50 {
            manager.fill_output(&mut output, 1.0);
        }
        // On the 50th starvation call, starvation_count resets to 0 and is_prebuffering = true.
        assert_eq!(manager.starvation_count, 0);
        assert!(manager.is_prebuffering);
    }

    #[test]
    fn test_fast_forward_udp_holes() {
        let (mut manager, mut encoder, mut prod, mut cons) = setup_env();
        let base_time = Instant::now();

        // Fill base tracking
        for i in 1..=MIN_DEPTH {
            assert!(
                prod.try_push(make_packet(&mut encoder, i as u64, base_time))
                    .is_ok()
            );
        }
        manager.ingest_packets(&mut cons);

        let mut output = vec![0.0; OPUS_FRAME_SAMPLES];
        for _ in 1..=MIN_DEPTH {
            manager.fill_output(&mut output, 1.0);
        }

        // Simulate a massive 10 packet UDP loss! We inject sequence 15 into the buffer,
        // while the playhead is currently looking for sequence (MIN_DEPTH + 1).
        let future_seq = MIN_DEPTH as u64 + 10;
        assert!(
            prod.try_push(make_packet(&mut encoder, future_seq, base_time))
                .is_ok()
        );
        manager.ingest_packets(&mut cons);

        // 1st missing frame: we wait (gap_hold_count increments, PLC output)
        // After REORDER_TOLERANCE waits, advance_one() fires and playhead advances past the gap.
        // The gap is 10 slots wide (beyond distance>20 threshold for large-gap fast-forward)
        // so fast_forward fires after advance_one resolves missing count > threshold.
        for _ in 0..REORDER_TOLERANCE {
            manager.fill_output(&mut output, 1.0);
        }

        // After REORDER_TOLERANCE calls, advance_one was called and missing_count incremented.
        assert_eq!(manager.missing_count, 1);
        assert!(!manager.is_prebuffering);
    }

    #[test]
    fn test_large_backlog_plays_without_crash() {
        let (mut manager, mut encoder, mut prod, mut cons) = setup_env();
        let base_time = Instant::now();

        // Push a very large burst of 160 packets (well above any internal threshold).
        for i in 1..=160u64 {
            assert!(
                prod.try_push(make_packet(&mut encoder, i, base_time))
                    .is_ok()
            );
        }
        manager.ingest_packets(&mut cons);

        // All 160 frames should be held in the jitter buffer.
        // fill_output should not panic or crash regardless of backlog size.
        let mut output = vec![0.0; OPUS_FRAME_SAMPLES];
        manager.fill_output(&mut output, 1.0);
        assert!(!manager.is_prebuffering);
    }

    #[test]
    fn test_ajb_hysteresis() {
        let (mut manager, mut encoder, mut prod, mut cons) = setup_env();
        let base_time = Instant::now();

        // Simulate 5 packets arriving at exactly the same time (burst jitter)
        for i in 1..=5 {
            let pcm = vec![0.0f32; OPUS_FRAME_SAMPLES];
            let opus_data = encoder.encode_vec_float(&pcm, 1500).unwrap();
            let pkt = RawPacket {
                seq_num: i as u64,
                payload_data: opus_data,
                arrival_time: base_time, // All arrived immediately
                is_uncompressed: false,
            };
            assert!(prod.try_push(pkt).is_ok());
        }
        manager.ingest_packets(&mut cons);

        // ema_jitter_ms should have spiked due to calculated variance vs expected times.
        // With 5 packets all at time 0 but expected at 0, 10, 20, 30, 40ms:
        // variance for each subsequent packet = |0 - 10| = 10ms, |0 - 10| = 10ms, etc.
        // EMA expands toward 10ms.
        assert!(
            manager.ema_jitter_ms > 0.0,
            "AJB ema_jitter_ms should be non-zero after burst"
        );
    }

    #[test]
    fn test_continuous_micro_jitter() {
        let (mut manager, mut encoder, mut prod, mut cons) = setup_env();
        let base_time = Instant::now();

        // Fill enough to exit prebuffering
        for i in 1..=MIN_DEPTH {
            assert!(
                prod.try_push(make_packet(&mut encoder, i as u64, base_time))
                    .is_ok()
            );
        }
        manager.ingest_packets(&mut cons);

        let mut output = vec![0.0; OPUS_FRAME_SAMPLES];
        for _ in 1..=MIN_DEPTH {
            manager.fill_output(&mut output, 1.0);
        }

        // At this point buffer is empty. Sequence is missing!
        // Frame 1 empty -> PLC
        manager.fill_output(&mut output, 1.0);
        assert_eq!(manager.starvation_count, 1);

        // Now packet 3 and 4 arrive (simulating 5GHz micro jitter delay batch arrival)
        assert!(
            prod.try_push(make_packet(&mut encoder, MIN_DEPTH as u64 + 1, base_time))
                .is_ok()
        );
        assert!(
            prod.try_push(make_packet(&mut encoder, MIN_DEPTH as u64 + 2, base_time))
                .is_ok()
        );
        manager.ingest_packets(&mut cons);

        // Frame 2 pop! We have packets in the buffer now!
        manager.fill_output(&mut output, 1.0);
        // Starvation count MUST reset because it was successfully mitigated!
        assert_eq!(manager.starvation_count, 0);
        // And is_prebuffering must NOT have triggered! Smooth streaming preserved!
        assert!(!manager.is_prebuffering);
    }

    #[test]
    fn test_extreme_macro_delay_three_seconds() {
        let (mut manager, mut encoder, mut prod, mut cons) = setup_env();
        let base_time = Instant::now();

        // 1. Initial network fill
        for i in 1..=MIN_DEPTH {
            assert!(
                prod.try_push(make_packet(&mut encoder, i as u64, base_time))
                    .is_ok()
            );
        }
        manager.ingest_packets(&mut cons);

        let mut output = vec![0.0; OPUS_FRAME_SAMPLES];
        for _ in 1..=MIN_DEPTH {
            manager.fill_output(&mut output, 1.0);
        }

        // 2. The 3 second Network Drop
        // We simulate 150 frames (3 seconds) of empty calls
        for _ in 1..=150 {
            manager.fill_output(&mut output, 1.0);
        }

        // The manager must be heavily in prebuffering mode, waiting out the extreme lag
        assert!(manager.is_prebuffering);

        // 3. Fresh batch arrives. ingest_packets directly inserts them (no flush).
        //    The jitter buffer is empty (starvation drained it), so it re-anchors at batch_start.
        let batch_start = MIN_DEPTH as u64 + 100;
        let batch_end = MIN_DEPTH as u64 + 150;
        for seq in batch_start..=batch_end {
            assert!(
                prod.try_push(make_packet(&mut encoder, seq, base_time))
                    .is_ok()
            );
        }
        manager.ingest_packets(&mut cons);

        // 4. fill_output: exits prebuffering (51 packets >=MIN_DEPTH), then sees a large gap
        //    (batch_start - old_next_play ≫ 20 frames) → large-gap fast_forward fires immediately.
        manager.fill_output(&mut output, 1.0);
        assert!(!manager.is_prebuffering);
        assert!(manager.buffer.next_play_seq() >= batch_start);
    }

    #[test]
    fn test_sender_crash_recovery() {
        let (mut manager, mut encoder, mut prod, mut cons) = setup_env();
        let base_time = Instant::now();

        // 1. Initial network fill (e.g. sequence 1000..1005)
        let early_seq = 1000;
        for i in 0..MIN_DEPTH {
            assert!(
                prod.try_push(make_packet(&mut encoder, early_seq + i as u64, base_time))
                    .is_ok()
            );
        }
        manager.ingest_packets(&mut cons);

        let mut output = vec![0.0; OPUS_FRAME_SAMPLES];
        for _ in 0..MIN_DEPTH {
            manager.fill_output(&mut output, 1.0);
        }

        // Assert we are playing around the 1000 mark!
        assert!(manager.buffer.next_play_seq() > 999);

        // 2. Android App force-crash and instantly restarts!
        // It starts sending sequence 0, 1, 2 again!
        for i in 0..MIN_DEPTH {
            assert!(
                prod.try_push(make_packet(&mut encoder, i as u64, base_time))
                    .is_ok()
            );
        }
        manager.ingest_packets(&mut cons);

        // The buffer physically detects that sequence 0 is > 128 packets BEHIND sequence 202.
        // It violently flushes its own timeline and re-anchors to 0!
        manager.fill_output(&mut output, 1.0);

        // The playhead must instantly snap back to 1!
        assert_eq!(manager.buffer.next_play_seq(), 1);
    }

    #[test]
    fn test_three_second_massive_jitter_burst() {
        let (mut manager, mut encoder, mut prod, mut cons) = setup_env();
        let base_time = Instant::now();

        for i in 1..=MIN_DEPTH {
            assert!(
                prod.try_push(make_packet(&mut encoder, i as u64, base_time))
                    .is_ok()
            );
        }
        manager.ingest_packets(&mut cons);

        let mut output = vec![0.0; OPUS_FRAME_SAMPLES];
        for _ in 1..=MIN_DEPTH {
            manager.fill_output(&mut output, 1.0);
        }

        // 3 seconds (300 frames) of pure silence due to massive Wi-Fi delay
        for _ in 1..=300 {
            manager.fill_output(&mut output, 1.0);
        }

        assert!(manager.is_prebuffering);

        // Fresh packets arrive. No flush needed (jitter buffer was empty).
        let batch_start = MIN_DEPTH as u64 + 1;
        let batch_end = MIN_DEPTH as u64 + 300;
        for seq in batch_start..=batch_end {
            assert!(
                prod.try_push(make_packet(&mut encoder, seq, base_time))
                    .is_ok()
            );
        }
        manager.ingest_packets(&mut cons);

        // fill_output: exits prebuffering (300 packets >= MIN_DEPTH).
        // next_play_seq was at MIN_DEPTH+1 = batch_start, so has_next() is true immediately.
        // No large-gap fast_forward needed since batch_start == next_play_seq.
        manager.fill_output(&mut output, 1.0);
        assert!(!manager.is_prebuffering);
        assert!(manager.buffer.next_play_seq() > batch_start);
    }
}
