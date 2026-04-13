use std::collections::VecDeque;

use opus::Decoder;
use ringbuf::{HeapCons, traits::*};

use crate::audio::{OPUS_CHANNELS, OPUS_FRAME_SAMPLES};

use super::buffer::JitterBuffer;
use super::types::RawPacket;

const MIN_DEPTH: u32 = 6;
const MAX_BUFFER_DEPTH: u32 = 25;
const MAX_MISSING: u32 = 100;

// AJB EMA Constants
const ALPHA_EXPAND: f32 = 0.2; // Fast expansion on spike
const ALPHA_SHRINK: f32 = 0.05; // Slow shrink on stable network

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

    ema_jitter_ms: f32,
    last_arrival: Option<Instant>,
    last_seq: Option<u64>,

    /// Stamping point for true NIC->DAC millisecond latency. Shared with receiver backend.
    latency_metric: Arc<AtomicU32>,
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
            ema_jitter_ms: 0.0,
            last_arrival: None,
            last_seq: None,
            latency_metric,
        }
    }

    /// Drain all pending raw packets from the SPSC channel into the jitter buffer.
    ///
    /// Called at the top of every cpal callback. The consumer is the audio-thread
    /// side of the lock-free ring buffer shared with the network thread.
    pub fn ingest_packets(&mut self, consumer: &mut HeapCons<RawPacket>) {
        while let Some(pkt) = consumer.try_pop() {
            if let (Some(last_arr), Some(last_s)) = (self.last_arrival, self.last_seq) {
                // Ignore severe out-of-order or restarts for jitter calc to prevent bad EMA spikes
                if pkt.seq_num > last_s && (pkt.seq_num - last_s) < 1000 {
                    let elapsed = pkt.arrival_time.duration_since(last_arr).as_millis() as f32;
                    let seq_delta = (pkt.seq_num - last_s) as f32;
                    let expected = seq_delta * 20.0; // 20ms per Opus frame (48kHz/960samples)

                    let variance = (elapsed - expected).abs();
                    let alpha = if variance > self.ema_jitter_ms {
                        ALPHA_EXPAND
                    } else {
                        ALPHA_SHRINK
                    };
                    self.ema_jitter_ms += alpha * (variance - self.ema_jitter_ms);
                }
            }

            self.last_arrival = Some(pkt.arrival_time);
            self.last_seq = Some(pkt.seq_num);

            self.buffer.insert(pkt);
        }
    }

    /// Fill `output` with PCM samples, applying jitter compensation.
    ///
    /// This is the cpal callback's main path. It will:
    /// 1. Pull from the internal playback buffer if data is available.
    /// 2. When the playback buffer is empty, process the next jitter buffer frame
    ///    (decode, time-stretch, or conceal) to refill it.
    /// 3. Scale each sample by `volume` (0.0–1.0).
    ///
    /// If we're in prebuffering mode (not enough depth yet), outputs silence.
    pub fn fill_output(&mut self, output: &mut [f32], volume: f32) {
        for sample in output.iter_mut() {
            if self.playback_buf.is_empty() {
                self.process_next_frame();
            }
            *sample = self.playback_buf.pop_front().unwrap_or(0.0) * volume;
        }
    }

    /// Process one Opus frame from the jitter buffer into the playback buffer.
    fn process_next_frame(&mut self) {
        // Exit prebuffering as soon as we have MIN_DEPTH frames.
        // Using the EMA-inflated target here caused 5 GHz latency to balloon
        // because ema_jitter_ms never fully decays to zero — even small variances
        // keep target_depth well above MIN_DEPTH indefinitely.
        if self.is_prebuffering {
            if self.buffer.occupied_count() >= MIN_DEPTH {
                self.is_prebuffering = false;
            } else {
                self.playback_buf
                    .extend(std::iter::repeat_n(0.0, OPUS_FRAME_SAMPLES));
                return;
            }
        }

        let depth = self.buffer.contiguous_depth();

        if depth > MAX_BUFFER_DEPTH + 5 {
            let catchup_target =
                self.buffer.next_play_seq() + depth.saturating_sub(MAX_BUFFER_DEPTH) as u64;
            self.buffer.fast_forward(catchup_target);
            self.generate_plc();
            return;
        }

        // pop_next() always advances next_play_seq even on a miss.
        // If we advance during a 2.4 GHz stall, burst packets arrive
        // with seq < next_play_seq and are silently rejected as stale.
        // We prevent that by short-circuiting here when the buffer is empty.
        if self.buffer.occupied_count() == 0 {
            self.missing_count += 1;
            self.starvation_count += 1;

            if self.missing_count > MAX_MISSING {
                self.trigger_reset();
                self.playback_buf
                    .extend(std::iter::repeat_n(0.0, OPUS_FRAME_SAMPLES));
                return;
            }

            // After 3 empty frames (~60ms), enter prebuffering to build cushion
            if self.starvation_count > 2 {
                self.is_prebuffering = true;
                self.starvation_count = 0;
                let _ = self.decoder.reset_state();
                self.playback_buf
                    .extend(std::iter::repeat_n(0.0, OPUS_FRAME_SAMPLES));
                return;
            }

            self.generate_plc();
            return;
        }

        if let Some(pkt) = self.buffer.pop_next() {
            self.missing_count = 0;
            self.starvation_count = 0;
            let delay_ms = Instant::now().duration_since(pkt.arrival_time).as_millis() as u32;
            self.latency_metric.store(delay_ms, Ordering::Relaxed);

            if pkt.is_uncompressed {
                self.buffer_uncompressed(&pkt.payload_data);
            } else {
                self.decode_and_buffer(&pkt.payload_data);
            }
        } else {
            // pop_next returned None but buffer IS occupied — a future packet exists,
            // meaning the current slot is a permanent UDP drop.
            self.missing_count += 1;
            self.starvation_count = 0;

            // Fast-forward past large gaps (> 3 frames) to reach available data
            if let Some(lowest) = self.buffer.lowest_available_seq() {
                let distance = lowest.saturating_sub(self.buffer.next_play_seq());
                if distance > 3 {
                    self.buffer.fast_forward(lowest);
                    let _ = self.decoder.reset_state();
                    self.playback_buf
                        .extend(std::iter::repeat_n(0.0, OPUS_FRAME_SAMPLES));
                    return;
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
    }

    fn trigger_reset(&mut self) {
        self.buffer.reset();
        self.is_prebuffering = true;
        self.missing_count = 0;
        self.starvation_count = 0;
        self.ema_jitter_ms = 0.0;
        self.last_arrival = None;
        self.last_seq = None;
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

        // Frame 3 empty -> Triggers Rebuffering margin!
        manager.fill_output(&mut output, 1.0);
        // Starvation count resets to 0 when it arms is_prebuffering
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

        // 1st missing frame hits Fast-Forward logic!
        manager.fill_output(&mut output, 1.0);

        // The playhead should instantly snap to 15!
        assert_eq!(manager.buffer.next_play_seq(), future_seq);

        // Fast forwarding doesn't reset the timeline anchor natively, just catches up
        assert!(!manager.is_prebuffering);
    }

    #[test]
    fn test_bufferbloat_protection() {
        let (mut manager, mut encoder, mut prod, mut cons) = setup_env();
        let base_time = Instant::now();

        // Push well above MAX_BUFFER_DEPTH to trigger the Catch-Up slice.
        let end_seq = MAX_BUFFER_DEPTH + 160;
        for i in 1..=end_seq {
            assert!(
                prod.try_push(make_packet(&mut encoder, i as u64, base_time))
                    .is_ok()
            );
        }
        manager.ingest_packets(&mut cons);

        let mut output = vec![0.0; OPUS_FRAME_SAMPLES];
        manager.fill_output(&mut output, 1.0);

        // CatchUp constraint fires unconditionally: manager seamlessly jumps its internal sequence pointer
        // to `occupied - MIN_DEPTH`, entirely dropping the huge lag segment!
        assert!(!manager.is_prebuffering);
        let distance_jumped = manager.buffer.next_play_seq().saturating_sub(1);
        assert!(
            distance_jumped > 30,
            "AJB Failed to fast-forward on massive backlog"
        );
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

        // ema_jitter_ms should have spiked way up due to calculated variance vs expected times
        assert!(
            manager.ema_jitter_ms > 10.0,
            "AJB ema_jitter_ms did not scale on burst"
        );
        let dynamic_target = (MIN_DEPTH as f32 + (manager.ema_jitter_ms / 20.0)).ceil() as u32;
        assert!(
            dynamic_target > MIN_DEPTH,
            "AJB dynamic target did not expand buffer capacity"
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

        // 3. The 3 second Backlog finally hits the socket!
        // The sender generated 150 frames, we receive frames 100 to 150
        // (assuming the OS UDP kernel buffer dropped the oldest 50 frames during the 3 second lag).
        let batch_start = MIN_DEPTH as u64 + 100;
        let batch_end = MIN_DEPTH as u64 + 150;
        for seq in batch_start..=batch_end {
            assert!(
                prod.try_push(make_packet(&mut encoder, seq, base_time))
                    .is_ok()
            );
        }
        manager.ingest_packets(&mut cons);

        // 4. We output the next frame. The massive backlog physically fills the Jitter Buffer!
        manager.fill_output(&mut output, 1.0);

        // At this point, the mathematical target_depth is satisfied.
        // Prebuffering smoothly disengages natively!
        assert!(!manager.is_prebuffering);

        // And the playhead Fast-Forwards natively across the huge UDP hole, jumping to 102!
        assert_eq!(manager.buffer.next_play_seq(), batch_start);
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

        // Network unstalls! All 300 packets arrive instantly in a single UDP flood!
        let batch_start = MIN_DEPTH as u64 + 1;
        let batch_end = MIN_DEPTH as u64 + 300;
        for seq in batch_start..=batch_end {
            assert!(
                prod.try_push(make_packet(&mut encoder, seq, base_time))
                    .is_ok()
            );
        }
        manager.ingest_packets(&mut cons);

        // Process the first frame of the massive burst
        manager.fill_output(&mut output, 1.0);

        // 1. Bufferbloat Slice logic perfectly triggers, snapping the UI forward
        assert!(!manager.is_prebuffering);

        // 2. Playhead slices through the backlog to catch up with 500ms bounds natively!
        // The playhead jumps straight to the live boundary!
        assert!(manager.buffer.next_play_seq() > batch_start + 250);
    }
}
