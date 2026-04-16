use std::collections::VecDeque;

use opus::Decoder;
use ringbuf::{HeapCons, traits::*};

use crate::audio::{OPUS_CHANNELS, OPUS_FRAME_SAMPLES, OPUS_FRAME_SIZE, OPUS_SAMPLE_RATE};

use super::buffer::JitterBuffer;
use super::types::RawPacket;

const MILLIS_PER_FRAME: u32 = (OPUS_FRAME_SIZE as u32 * 1000) / OPUS_SAMPLE_RATE;

/// Absolute floor of 20ms.
/// The adaptive `prebuffer_target` provides the real safety net above this.
const MIN_DEPTH: u32 = 20 / MILLIS_PER_FRAME;
/// 2000ms max silence before resetting stream
const MAX_MISSING: u32 = 2000 / MILLIS_PER_FRAME;

/// Reorder tolerance: ~30ms window to wait for a reordered packet.
const REORDER_TOLERANCE: u32 = 30 / MILLIS_PER_FRAME;

/// Hard flush ceiling — frames above the current prebuffer target before Tier 2 fires.
/// 40ms of extra headroom. The buffer is never touched below this line.
/// Only catastrophic clock drift or prolonged OS UI freezes will trigger a flush.
const MAX_HEADROOM: u32 = 40 / MILLIS_PER_FRAME;

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
    
    /// Tracks the exact sequence number the Opus predictive state machine is calibrated for.
    opus_next_expected_seq: Option<u64>,

    /// The frame depth the buffer must reach before exiting prebuffering.
    /// Set by `compute_prebuffer_target()` each time starvation triggers a rebuffer.
    /// Predictive: it adapts to the learned network burst size so we never exit
    /// prebuffering into a network that will immediately re-starve.
    prebuffer_target: u32,

    last_network_arrival: Option<Instant>,

    /// Stamping point for true NIC->DAC millisecond latency. Shared with receiver backend.
    latency_metric: Arc<AtomicU32>,
    /// How many consecutive callbacks we've been waiting for the current gap slot.
    /// Prevents spurious PLC for late-arriving reordered packets on 2.4GHz.
    gap_hold_count: u32,

    /// Per-cpal-callback gate: set to `true` by `ingest_packets` (called once per
    /// audio callback) and cleared to `false` the moment a WSOLA shed fires.
    wsola_allowed_this_tick: bool,

    /// Current maximum observed burst gaps. The peak defines the active safety floor.
    peak_burst_f32: f32,

    /// 30-bucket sliding window (1s intervals) to preserve bursts for 30 seconds.
    peak_history: VecDeque<f32>,
    current_second_peak: f32,
    last_history_push: Option<Instant>,
    consecutive_clean_seconds: u32,
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
            opus_next_expected_seq: None,
            prebuffer_target: MIN_DEPTH,
            last_network_arrival: None,
            latency_metric,
            gap_hold_count: 0,
            wsola_allowed_this_tick: false,
            peak_burst_f32: MIN_DEPTH as f32,
            peak_history: VecDeque::with_capacity(30),
            current_second_peak: MIN_DEPTH as f32,
            last_history_push: None,
            consecutive_clean_seconds: 0,
        }
    }

    /// Drain all pending raw packets from the SPSC channel into the jitter buffer.
    ///
    /// Called at the top of every cpal callback. The consumer is the audio-thread
    /// side of the lock-free ring buffer shared with the network thread.
    pub fn ingest_packets(&mut self, consumer: &mut HeapCons<RawPacket>) {
        self.wsola_allowed_this_tick = true;
        let now = Instant::now();
        
        let mut max_gap_ms = 0;
        let mut packets_received = 0;

        while let Some(pkt) = consumer.try_pop() {
            if let Some(last) = self.last_network_arrival {
                if pkt.arrival_time > last {
                    let gap = pkt.arrival_time.duration_since(last).as_millis() as u32;
                    let gap_clamped = gap.min(10000);
                    if gap_clamped > max_gap_ms {
                        max_gap_ms = gap_clamped;
                    }
                }
            } else {
                max_gap_ms = 0; // First packet ever
            }
            self.last_network_arrival = Some(pkt.arrival_time);
            self.buffer.insert(pkt);
            packets_received += 1;
        }

        // Push every 1 second into a 30-bucket sliding window.
        if let Some(last_push) = self.last_history_push {
            if now.duration_since(last_push).as_millis() >= 1000 {
                let finalized_peak = self.current_second_peak;

                if self.peak_history.len() >= 30 {
                    self.peak_history.pop_front();
                }
                self.peak_history.push_back(finalized_peak);
                self.current_second_peak = MIN_DEPTH as f32; // reset
                self.last_history_push = Some(now);

                // Consecutive clean heuristic for agility
                // 24 frames = 120ms, safely covers Android's native 100ms Wi-Fi batching duration
                let clean_threshold = (MIN_DEPTH as f32 * 6.0).max(24.0);
                if finalized_peak < clean_threshold {
                    self.consecutive_clean_seconds += 1;
                } else {
                    self.consecutive_clean_seconds = 0;
                }

                // Only aggressively strip latency overhead when it's genuinely high (e.g. > 150ms).
                // If it's already below 150ms, let the 30-sec sliding window handle decay natively.
                if self.consecutive_clean_seconds >= 5 && self.peak_burst_f32 > 30.0 {
                    for bucket in &mut self.peak_history {
                        *bucket = (*bucket * 0.8).max(MIN_DEPTH as f32);
                    }
                }
            }
        } else {
            self.last_history_push = Some(now);
        }

        if packets_received > 0 {
            let gap_frames = max_gap_ms as f32 / MILLIS_PER_FRAME as f32;
            let gap_as_risk = gap_frames.max(MIN_DEPTH as f32);
            if gap_as_risk > self.current_second_peak {
                self.current_second_peak = gap_as_risk;
            }
        }

        // Evaluate live peak tracker.
        self.peak_burst_f32 = self
            .peak_history
            .iter()
            .copied()
            .fold(self.current_second_peak, f32::max);

        let safe_ema = (self.peak_burst_f32 as u32).max(MIN_DEPTH);
        self.prebuffer_target = safe_ema.min(300); // Wait 6 seconds max
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
        let safe_floor = (self.peak_burst_f32.ceil() as u32).max(MIN_DEPTH);

        // Tier 2: decode-and-discard flush. Only fires when the buffer has grown
        // far beyond the current prebuffer target (e.g. after a prolonged GC freeze or
        // connection burst). We deliberately decode each discarded packet so the Opus
        // decoder keeps its internal state in sync — skipping via advance_one() without
        // decoding causes the next real frame to decode with a stale prediction model,
        // producing a harsh click/pop artifact.
        let flush_ceiling = self.prebuffer_target.max(safe_floor) + MAX_HEADROOM;
        if self.buffer.occupied_count() > flush_ceiling {
            let flush_target = self.prebuffer_target.max(safe_floor);
            while self.buffer.occupied_count() > flush_target {
                if let Some(pkt) = self.buffer.pop_next() {
                    // Decode and throw away — just to keep the Opus state machine synced.
                    let _ = self.capture_pcm(&pkt);
                } else {
                    self.buffer.advance_one();
                }
            }
        }

        if self.is_prebuffering {
            // Un-mute playback progressively during a recovery phase.
            // Must buffer a substantial chunk (up to 500ms) to avoid "machine-gun" 
            // windmill stuttering when packets trickle in over a congested 2.4GHz link.
            let unpause_threshold = self.prebuffer_target.min(100).max(MIN_DEPTH);
            if self.buffer.occupied_count() >= unpause_threshold {
                self.is_prebuffering = false;
            } else {
                self.generate_plc();
                return;
            }
        }

        if self.buffer.has_next() {
            self.gap_hold_count = 0;
            self.missing_count = 0;
            self.starvation_count = 0;

            let pkt = self.buffer.pop_next().expect("has_next was true");
            let delay_ms = Instant::now().duration_since(pkt.arrival_time).as_millis() as u32;
            self.latency_metric.store(delay_ms, Ordering::Relaxed);

            let pcm = self.capture_pcm(&pkt);
            // Margin: breathing room above the safe network peak.
            // Dynamically collapses from 40ms to 20ms during stable 5GHz playback to crush latency.
            let proportional_margin = (safe_floor as f32 * 0.15).ceil() as u32;
            let base_margin = if self.consecutive_clean_seconds >= 5 {
                MIN_DEPTH.max(2)
            } else {
                MIN_DEPTH * 2
            };
            let margin = base_margin.max(proportional_margin).min(100);

            // The peak burst tracker IS the minimum safe buffer depth — it represents
            // the largest burst the network has recently delivered. Never shed below
            // this or we'll starve during the next observed burst of equal magnitude.
            // Unlike ema_jitter_ms (which decays quickly), peak_burst_f32 stays
            // elevated for ~30s, providing lasting protection against recurring patterns.

            // Only shed frames that sit above the safety floor plus a small breathing margin.
            let effective_threshold = safe_floor + margin;

            if self.buffer.occupied_count() > effective_threshold && !self.is_prebuffering {
                let rms = Self::get_rms(&pcm);

                // How many frames above the hard safety floor are we?
                // Drives max_skip_frames so compression is gentle near the floor
                // and proportionally stronger when deeply bloated.
                let safe_excess = self
                    .buffer
                    .occupied_count()
                    .saturating_sub(effective_threshold);

                if rms < 0.005 {
                    // Silence: drop packet outright — no crossfade needed.
                    return;
                } else if self.wsola_allowed_this_tick && self.buffer.has_next() {
                    // Smooth, proportional WSOLA gliding.
                    // Far above target = cap at 12 frames (5% tempo increase) to avoid robotic clipping over-compression.
                    // Near target = soft 1-frame drops to perfectly glide into place without audible distortion.
                    let proportional = ((safe_excess as f32) * 1.5).ceil() as usize;
                    let max_skip_frames = proportional.clamp(1, 12);

                    let pkt2 = self.buffer.pop_next().unwrap();
                    let delay2_ms =
                        Instant::now().duration_since(pkt2.arrival_time).as_millis() as u32;
                    self.latency_metric.store(delay2_ms, Ordering::Relaxed);

                    let pcm2 = self.capture_pcm(&pkt2);
                    self.wsola_allowed_this_tick = false; // Consume tick budget until next ingest
                    self.forward_wsola_shed(&pcm, &pcm2, max_skip_frames);
                    return;
                }
            }

            self.playback_buf.extend(pcm);
            return;
        }

        if self.buffer.occupied_count() == 0 {
            self.gap_hold_count = 0;
            self.missing_count += 1;
            self.starvation_count += 1;

            if self.missing_count > MAX_MISSING {
                self.trigger_reset();
                self.playback_buf
                    .extend(std::iter::repeat_n(0.0, OPUS_FRAME_SAMPLES));
                return;
            }

            // Re-anchor after 5 consecutive starved frames (50ms).
            // Recompute the prebuffer target NOW so we re-enter prebuffering
            // with the correct depth for the current network conditions.
            if self.starvation_count >= 300 / MILLIS_PER_FRAME {
                self.is_prebuffering = true;
                self.starvation_count = 0;
                self.gap_hold_count = 0;
            }

            self.generate_plc();
            return;
        }

        self.starvation_count = 0;

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
                    // Reset is now safely handled natively upon the NEXT ingest by capture_pcm() tracking!
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
        self.wsola_allowed_this_tick = false;
        self.playback_buf.clear();
        self.decode_buf.fill(0.0);
        self.decode_len = 0;
        let _ = self.decoder.reset_state();
    }

    fn get_rms(samples: &[f32]) -> f32 {
        let mut sum_sq = 0.0;
        for &s in samples {
            sum_sq += s * s;
        }
        (sum_sq / samples.len() as f32).sqrt()
    }

    fn capture_pcm(&mut self, pkt: &RawPacket) -> Vec<f32> {
        if let Some(expected) = self.opus_next_expected_seq {
            if pkt.seq_num != expected {
                // Catastrophic Opus misalignment (skipped frames). Reset predictive engine!
                let _ = self.decoder.reset_state();
            }
        }

        let pcm = if pkt.is_uncompressed {
            let f32_len = pkt.payload_data.len() / std::mem::size_of::<f32>();
            let mut temp_samples = Vec::with_capacity(f32_len);
            for chunk in pkt.payload_data.chunks_exact(4) {
                let f = f32::from_ne_bytes(chunk.try_into().unwrap());
                temp_samples.push(f);
            }
            let _ = self.decoder.decode_float(&[], &mut self.decode_buf, false);
            if temp_samples.is_empty() {
                self.decode_plc_to_buf();
                self.decode_buf[..self.decode_len].to_vec()
            } else {
                temp_samples
            }
        } else {
            if self.decode_opus(&pkt.payload_data) {
                self.decode_buf[..self.decode_len].to_vec()
            } else {
                self.decode_plc_to_buf();
                self.decode_buf[..self.decode_len].to_vec()
            }
        };

        self.opus_next_expected_seq = Some(pkt.seq_num + 1);
        pcm
    }

    /// WSOLA splice: discard exactly `max_skip_frames` sample-frames at the junction
    /// of two decoded Opus packets, using SAD to find the best phase-aligned splice
    /// point within a limited search window.
    ///
    /// **Geometry** (all units = stereo sample frames):
    ///   Input:  pcm1 (n frames) ++ pcm2 (n frames) = 2n frames
    ///   Output: (n - w) verbatim from pcm1
    ///         + w crossfade
    ///         + (n - best_offset - w) from pcm2
    ///   Saved:  best_offset + w ≤ max_skip_frames
    ///
    /// At 48kHz, n=240 (5ms), max_skip=24 → max savings = 0.5ms per call.
    /// With wsola_allowed_this_tick limiting to 1 call per ~42ms cpal callback,
    /// the ceiling compression rate is ~12ms/sec — safe for all observed networks.
    fn forward_wsola_shed(&mut self, pcm1: &[f32], pcm2: &[f32], max_skip_frames: usize) {
        let channels = OPUS_CHANNELS as usize;
        let n = pcm1.len() / channels; // sample frames per packet (240 at 5ms/48kHz)

        if max_skip_frames == 0 || n < max_skip_frames * 4 {
            // Guard: not enough data for a meaningful crossfade, just append verbatim.
            self.playback_buf.extend(pcm1);
            return;
        }

        // Crossfade window = half the skip budget (minimum 1 frame).
        // The other half is the search range for phase alignment.
        let w = (max_skip_frames / 2).max(1);
        // search_end: how many extra frames into pcm2 we may skip (beyond the crossfade).
        let search_end = max_skip_frames.saturating_sub(w);

        // Find the alignment offset in pcm2[0..search_end] that minimises SAD with
        // the tail of pcm1[n-w..n]. Minimum offset = 0 means "no extra skip, just
        // crossfade at the natural packet boundary."
        let mut best_offset = 0usize;
        let mut min_sad = f32::MAX;
        for offset in 0..=search_end {
            let mut sad = 0.0f32;
            for i in 0..w {
                for c in 0..channels {
                    let i_p = (n.saturating_sub(w) + i) * channels + c;
                    let i_q = (offset + i) * channels + c;
                    let p = if i_p < pcm1.len() { pcm1[i_p] } else { 0.0 };
                    let q = if i_q < pcm2.len() { pcm2[i_q] } else { 0.0 };
                    sad += (p - q).abs();
                }
            }
            if sad < min_sad {
                min_sad = sad;
                best_offset = offset;
            }
        }

        // ── Output stage ──────────────────────────────────────────────────────
        // 1. pcm1[0..n-w] verbatim
        for f in 0..(n - w) {
            for c in 0..channels {
                self.playback_buf.push_back(pcm1[f * channels + c]);
            }
        }

        // 2. Crossfade: pcm1[n-w..n] fades out, pcm2[best_offset..+w] fades in.
        for i in 0..w {
            let alpha = i as f32 / w.max(1) as f32;
            let fade_in = (alpha * std::f32::consts::FRAC_PI_2).sin();
            let fade_out = (alpha * std::f32::consts::FRAC_PI_2).cos();
            for c in 0..channels {
                let p = pcm1[(n - w + i) * channels + c];
                let qi = (best_offset + i) * channels + c;
                let q = if qi < pcm2.len() { pcm2[qi] } else { 0.0 };
                self.playback_buf.push_back(p * fade_out + q * fade_in);
            }
        }

        // 3. pcm2[best_offset+w..n] verbatim (rest of second packet, tail only).
        for f in (best_offset + w)..n {
            for c in 0..channels {
                let idx = f * channels + c;
                if idx < pcm2.len() {
                    self.playback_buf.push_back(pcm2[idx]);
                }
            }
        }
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

    /// Use Opus's native PLC to hallucinate a missing frame to `decode_buf`.
    fn decode_plc_to_buf(&mut self) {
        match self
            .decoder
            .decode_float(&[] as &[u8], &mut self.decode_buf, false)
        {
            Ok(samples_per_channel) => {
                self.decode_len = samples_per_channel * OPUS_CHANNELS as usize;
            }
            Err(_) => {
                self.decode_buf.fill(0.0);
                self.decode_len = OPUS_FRAME_SAMPLES;
            }
        }
    }

    /// Internal generator for when NO valid packet is pulled at all.
    fn generate_plc(&mut self) {
        self.decode_plc_to_buf();
        self.playback_buf.extend(&self.decode_buf[..self.decode_len]);
        if let Some(expected) = self.opus_next_expected_seq {
            self.opus_next_expected_seq = Some(expected + 1);
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

        // Drain exactly 60 starvation frames (300ms) to hit the >= 60 threshold.
        for _ in 3..=60 {
            manager.fill_output(&mut output, 1.0);
        }
        // On the 60th starvation call, starvation_count resets to 0 and is_prebuffering = true.
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
        let batch_end = MIN_DEPTH as u64 + 250;
        for seq in batch_start..=batch_end {
            assert!(
                prod.try_push(make_packet(&mut encoder, seq, base_time))
                    .is_ok()
            );
        }
        manager.ingest_packets(&mut cons);

        // 4. fill_output: exits prebuffering (151 packets >= 100 limit), then sees a large gap
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


}
