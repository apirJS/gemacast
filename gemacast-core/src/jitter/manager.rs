use std::collections::VecDeque;

use opus::Decoder;
use ringbuf::{HeapCons, traits::*};

use crate::audio::{OPUS_CHANNELS, OPUS_FRAME_SAMPLES, OPUS_FRAME_SIZE, OPUS_SAMPLE_RATE};
use crate::types::JitterConfig;

use super::buffer::JitterBuffer;
use super::types::RawPacket;

const MILLIS_PER_FRAME: u32 = (OPUS_FRAME_SIZE as u32 * 1000) / OPUS_SAMPLE_RATE;

/// 2000ms max silence before resetting stream
const MAX_MISSING: u32 = 2000 / MILLIS_PER_FRAME;

/// Reorder tolerance: ~30ms window to wait for a reordered packet.
const REORDER_TOLERANCE: u32 = 30 / MILLIS_PER_FRAME;

use std::sync::RwLock;
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

    last_network_arrival: Option<Instant>,

    /// Stamping point for true NIC->DAC millisecond latency. Shared with receiver backend.
    latency_metric: Arc<AtomicU32>,
    /// How many consecutive callbacks we've been waiting for the current gap slot.
    /// Prevents spurious PLC for late-arriving reordered packets on 2.4GHz.
    gap_hold_count: u32,

    /// Per-cpal-callback gate: set to `true` by `ingest_packets` (called once per
    /// audio callback) and cleared to `false` the moment a WSOLA shed fires.
    wsola_allowed_this_tick: bool,

    /// Continuous Mathematical Spring tracking the lowest comfort point dynamically.
    /// Grows instantly upon network starvation bursts, bleeds down mechanically.
    comfort_point_frames: f32,

    /// Countdown: how many frames since last reset.
    /// During `fast_settle_frames`, bleed rate is multiplied for rapid convergence.
    frames_since_reset: u32,

    config: JitterConfig,
    config_ref: Arc<RwLock<JitterConfig>>,
}

impl JitterBufferManager {
    /// Convert milliseconds to frames using ceiling division.
    /// Prevents truncation to 0 for sub-frame values (e.g. 2ms / 5ms = 1 frame, not 0).
    fn ms_to_frames_ceil(ms: u32) -> u32 {
        (ms + MILLIS_PER_FRAME - 1) / MILLIS_PER_FRAME
    }

    pub fn new(decoder: Decoder, latency_metric: Arc<AtomicU32>, config_ref: Arc<RwLock<JitterConfig>>) -> Self {
        let initial_config = config_ref.read().unwrap().clone();
        // Seed comfort at the preset's initial_comfort_ms instead of min_depth.
        // This eliminates the bounce overshoot on first connect.
        let initial_comfort = Self::ms_to_frames_ceil(initial_config.initial_comfort_ms)
            .max(Self::ms_to_frames_ceil(initial_config.min_depth_ms)) as f32;
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
            last_network_arrival: None,
            latency_metric,
            gap_hold_count: 0,
            wsola_allowed_this_tick: false,
            comfort_point_frames: initial_comfort,
            frames_since_reset: 0,
            config: initial_config,
            config_ref,
        }
    }

    fn min_depth_frames(&self) -> u32 {
        self.config.min_depth_ms / MILLIS_PER_FRAME
    }

    fn comfort_cap_frames(&self) -> f32 {
        (self.config.comfort_cap_ms / MILLIS_PER_FRAME) as f32
    }

    /// Drain all pending raw packets from the SPSC channel into the jitter buffer.
    ///
    /// Called at the top of every cpal callback. The consumer is the audio-thread
    /// side of the lock-free ring buffer shared with the network thread.
    pub fn ingest_packets(&mut self, consumer: &mut HeapCons<RawPacket>) {
        let mut received = false;

        while let Some(pkt) = consumer.try_pop() {
            self.last_network_arrival = Some(pkt.arrival_time);
            self.buffer.insert(pkt);
            received = true;
        }

        // Only allow WSOLA shedding when new data actually arrived.
        // During network blackouts, the buffer must drain at 1x rate to
        // maximize survival time. Previously WSOLA fired every audio callback
        // even with zero new packets, draining the buffer ~25% faster.
        self.wsola_allowed_this_tick = received;
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
        // === The Comfy Bouncer: Starvation-Duration Adaptive Algorithm ===
        //
        // Tracks how deep the buffer needs to be to survive the current network's worst jitter.
        //   1. BLEED (downward): Constantly probes for lower latency.
        //   2. BOUNCE (upward): Grows dynamically upon starvation bursts.

        self.frames_since_reset += 1;

        // Check for config changes (read-only borrow of config_ref).
        // The flush must happen AFTER the guard is dropped to satisfy the borrow checker.
        let mut pending_flush: Option<u32> = None;

        if let Ok(guard) = self.config_ref.try_read() {
            let new_config = guard.clone();
            // When config changes (e.g. user switches preset), snap comfort to the
            // new preset's initial_comfort_ms for an instant, clean transition.
            // Reset the fast-settle counter so the system re-converges quickly.
            if new_config != self.config {
                let new_initial = Self::ms_to_frames_ceil(new_config.initial_comfort_ms)
                    .max(Self::ms_to_frames_ceil(new_config.min_depth_ms)) as f32;
                let new_cap = Self::ms_to_frames_ceil(new_config.comfort_cap_ms) as f32;
                eprintln!("[JitterManager] Config changed! initial_comfort={}ms({}f) comfort_cap={}ms({}f) old_comfort_point={:.1}f",
                    new_config.initial_comfort_ms, new_initial, new_config.comfort_cap_ms, new_cap, self.comfort_point_frames);
                self.comfort_point_frames = new_initial.clamp(new_initial, new_cap);
                self.frames_since_reset = 0; // Re-enter fast-settle for rapid convergence
                self.is_prebuffering = true; // Brief mute for clean transition

                // Compute flush target for after the guard is dropped.
                let new_safe_floor = (new_initial.ceil() as u32).max(Self::ms_to_frames_ceil(new_config.min_depth_ms));
                let flush_target = new_safe_floor + new_safe_floor / 2; // Keep 50% margin
                if self.buffer.occupied_count() > flush_target {
                    pending_flush = Some(flush_target);
                }

                eprintln!("[JitterManager] comfort_point_frames snapped to {:.1}f, fast-settle re-engaged", self.comfort_point_frames);
                self.config = new_config;
            }
        } // guard dropped here

        // Aggressive buffer flush: if switching to a lower preset, immediately
        // drain excess packets instead of waiting for WSOLA to slowly shed.
        if let Some(flush_target) = pending_flush {
            eprintln!("[JitterManager] Flushing {} excess frames (occupied={} target={})",
                self.buffer.occupied_count().saturating_sub(flush_target),
                self.buffer.occupied_count(), flush_target);
            while self.buffer.occupied_count() > flush_target {
                if let Some(pkt) = self.buffer.pop_next() {
                    let _ = self.capture_pcm(&pkt);
                } else {
                    self.buffer.advance_one();
                }
            }
        }

        let min_depth = self.min_depth_frames();
        let comfort_cap = self.comfort_cap_frames();

        // Adaptive bleed rate pushes comfort_point_frames down towards MIN_DEPTH.
        // During the fast-settle window, multiply for rapid convergence to optimal.
        let settle_boost = if self.frames_since_reset < self.config.fast_settle_frames {
            self.config.fast_settle_multiplier
        } else {
            1.0
        };
        let bleed = (0.04 * min_depth as f32 / self.comfort_point_frames).clamp(0.005, 0.04) * settle_boost;
        self.comfort_point_frames = (self.comfort_point_frames - bleed).max(min_depth as f32);

        let safe_floor = (self.comfort_point_frames.ceil() as u32).max(min_depth);

        // Tier 2: hard flush. Only flush when buffer is WAY above comfort.
        // Fixed 1000ms headroom. Flush target retains 500ms of margin above
        // comfort so we don't immediately starve after flushing.
        let flush_ceiling = safe_floor + 200;
        let flush_target = safe_floor + 100; // Keep 500ms margin after flush
        if self.buffer.occupied_count() > flush_ceiling {
            while self.buffer.occupied_count() > flush_target {
                if let Some(pkt) = self.buffer.pop_next() {
                    let _ = self.capture_pcm(&pkt);
                } else {
                    self.buffer.advance_one();
                }
            }
        }

        if self.is_prebuffering {
            // Resume based on resume_threshold_pct (e.g., 75% of comfort depth).
            let unpause_threshold = ((safe_floor as f32 * self.config.resume_threshold_pct) as u32).max(min_depth);
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

            if self.starvation_count > 0 {
                let bounce = self.starvation_count as f32 * self.config.bounce_multiplier;
                self.comfort_point_frames = (self.comfort_point_frames + bounce).min(comfort_cap);
                self.starvation_count = 0;
                // Re-engage fast-settle so the system aggressively bleeds back
                // down after Wi-Fi power-save bursts instead of crawling.
                self.frames_since_reset = 0;
            }

            let pkt = self.buffer.pop_next().expect("has_next was true");
            let delay_ms = Instant::now().duration_since(pkt.arrival_time).as_millis() as u32;
            self.latency_metric.store(delay_ms, Ordering::Relaxed);

            let pcm = self.capture_pcm(&pkt);

            // WSOLA shedding: engage when buffer is above comfort + 20% margin.
            // A smaller margin ensures the shedder actively reduces latency after
            // Wi-Fi power-save bursts (screen off) without eating into safety cushion.
            let current_floor = (self.comfort_point_frames.ceil() as u32).max(min_depth);
            let effective_threshold = current_floor + current_floor / 5;

            if self.buffer.occupied_count() > effective_threshold {
                let rms = Self::get_rms(&pcm);
                let safe_excess = self
                    .buffer
                    .occupied_count()
                    .saturating_sub(effective_threshold);

                if rms < 0.005 {
                    return;
                } else if self.wsola_allowed_this_tick && self.buffer.has_next() {
                    // Gentle WSOLA: cap at maximum skip frames.
                    let proportional = ((safe_excess as f32) * 0.5).ceil() as usize;
                    let max_skip_frames = proportional.clamp(1, self.config.wsola_max_skip.max(1));

                    let pkt2 = self.buffer.pop_next().unwrap();
                    let delay2_ms =
                        Instant::now().duration_since(pkt2.arrival_time).as_millis() as u32;
                    self.latency_metric.store(delay2_ms, Ordering::Relaxed);

                    let pcm2 = self.capture_pcm(&pkt2);
                    self.wsola_allowed_this_tick = false;
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

            // Enter prebuffering after sustained starvation (50ms).
            // This gives the bounce a chance to accumulate before we try playing.
            if self.starvation_count >= 10 {
                self.is_prebuffering = true;
            }

            self.generate_plc();
            return;
        }

        // Gap path: packets exist but not for the current sequence.
        // Also bounce if we were starved before hitting this path.
        if self.starvation_count > 0 {
            let bounce = self.starvation_count as f32 * self.config.bounce_multiplier;
            self.comfort_point_frames = (self.comfort_point_frames + bounce).min(comfort_cap);
            // Re-engage fast-settle after gap-path bounce too.
            self.frames_since_reset = 0;
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
        self.missing_count = 0;
        self.starvation_count = 0;
        self.gap_hold_count = 0;
        self.wsola_allowed_this_tick = false;
        self.playback_buf.clear();
        self.decode_buf.fill(0.0);
        self.decode_len = 0;
        let _ = self.decoder.reset_state();
        // Seed comfort at the preset's initial comfort instead of min_depth.
        // This eliminates the bounce overshoot that causes ~200ms initial latency.
        let initial_comfort = Self::ms_to_frames_ceil(self.config.initial_comfort_ms)
            .max(Self::ms_to_frames_ceil(self.config.min_depth_ms)) as f32;
        self.comfort_point_frames = initial_comfort;
        self.frames_since_reset = 0; // Re-engage fast-settle window
    }

    fn get_rms(samples: &[f32]) -> f32 {
        let mut sum_sq = 0.0;
        for &s in samples {
            sum_sq += s * s;
        }
        (sum_sq / samples.len() as f32).sqrt()
    }

    fn capture_pcm(&mut self, pkt: &RawPacket) -> Vec<f32> {
        if let Some(expected) = self.opus_next_expected_seq
            && pkt.seq_num != expected
        {
            // Catastrophic Opus misalignment (skipped frames). Reset predictive engine!
            let _ = self.decoder.reset_state();
        }

        let pcm = if pkt.is_silence {
            let _ = self.decoder.decode_float(&[], &mut self.decode_buf, false);
            vec![0.0f32; OPUS_FRAME_SAMPLES]
        } else if pkt.is_uncompressed {
            let f32_len = pkt.payload_len / std::mem::size_of::<f32>();
            let mut temp_samples = Vec::with_capacity(f32_len);
            for chunk in pkt.payload_data[..pkt.payload_len].chunks_exact(4) {
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
            if self.decode_opus(&pkt.payload_data[..pkt.payload_len]) {
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
        self.playback_buf
            .extend(&self.decode_buf[..self.decode_len]);
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

    /// Must match JitterConfig::default().min_depth_ms / MILLIS_PER_FRAME (40ms / 5ms = 8 frames).
    const MIN_DEPTH: u32 = 8;

    fn setup_env() -> (
        JitterBufferManager,
        Encoder,
        ringbuf::HeapProd<RawPacket>,
        ringbuf::HeapCons<RawPacket>,
    ) {
        let decoder = Decoder::new(OPUS_SAMPLE_RATE, Channels::Stereo).unwrap();
        let encoder = Encoder::new(OPUS_SAMPLE_RATE, Channels::Stereo, Application::Audio).unwrap();
        let atomic = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let config_ref = Arc::new(std::sync::RwLock::new(JitterConfig::default()));
        let manager = JitterBufferManager::new(decoder, atomic, config_ref);
        let rb = HeapRb::<RawPacket>::new(1000);
        let (prod, cons) = rb.split();
        (manager, encoder, prod, cons)
    }

    fn make_packet(encoder: &mut Encoder, seq: u64, base_time: Instant) -> RawPacket {
        let pcm = vec![0.0f32; OPUS_FRAME_SAMPLES];
        let d = encoder.encode_vec_float(&pcm, 1500).unwrap();
        let payload_len = d.len();
        let payload_data = d.clone();
        RawPacket {
            seq_num: seq,
            payload_data,
            payload_len,
            arrival_time: base_time + std::time::Duration::from_millis(seq * 20),
            is_uncompressed: false,
            is_silence: false,
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

        // Drain to exactly 10 starvation frames (50ms) to hit the >= 10 threshold.
        for _ in 3..=10 {
            manager.fill_output(&mut output, 1.0);
        }
        // On the 10th starvation call, is_prebuffering = true.
        // starvation_count is preserved (not reset) so the bounce can use it later.
        assert_eq!(manager.starvation_count, 10);
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
