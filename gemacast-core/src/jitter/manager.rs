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

use std::sync::Arc;
use std::sync::RwLock;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Instant;

/// OLA window length in sample-frames for WSOLA crossfading.
/// 128 frames = 2.67ms at 48kHz — long enough for perceptual transparency.
const OLA_LEN: usize = 128;

/// Search range in sample-frames for cross-correlation alignment.
const SEARCH_RANGE: usize = 128;

/// Coordinates the full jitter buffer pipeline.
///
/// Owns the buffer and Opus decoder. Runs entirely within the cpal audio callback thread.
/// Communication with the network thread happens via the lock-free SPSC `HeapCons`.
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

    /// EWMA of inter-arrival jitter (frames).
    ema_jitter: f32,
    /// Slow-decay peak tracker for worst-case jitter (frames).
    ema_peak: f32,
    /// Additive target bump after starvation, bleeds continuously.
    starvation_bump: f32,

    /// Last ingested sequence number to detect consecutive packets for IAT.
    last_ingest_seq: Option<u64>,

    /// Countdown for continuous startup flush.
    startup_flush_remaining: u32,
    ema_peak_decay_alpha: f32,

    /// When the last major jitter spike (>50ms) occurred
    last_macro_spike: Option<Instant>,
    /// Unstable network (e.g. 2.4GHz scan cycle) regime expiration.
    unstable_regime_until: Option<Instant>,

    config: JitterConfig,
    config_ref: Arc<RwLock<JitterConfig>>,
    is_tcp_mode: Arc<AtomicBool>,

    /// Pre-computed Hann window for OLA crossfading (OLA_LEN entries).
    hann_window: Vec<f32>,
}

impl JitterBufferManager {
    /// Convert milliseconds to frames using ceiling division.
    /// Prevents truncation to 0 for sub-frame values (e.g. 2ms / 5ms = 1 frame, not 0).
    fn ms_to_frames_ceil(ms: u32) -> u32 {
        ms.div_ceil(MILLIS_PER_FRAME)
    }

    fn make_hann_window() -> Vec<f32> {
        (0..OLA_LEN)
            .map(|i| 0.5 * (1.0 - (2.0 * std::f32::consts::PI * i as f32 / OLA_LEN as f32).cos()))
            .collect()
    }

    pub fn new(
        decoder: Decoder,
        latency_metric: Arc<AtomicU32>,
        config_ref: Arc<RwLock<JitterConfig>>,
        is_tcp_mode: Arc<AtomicBool>,
    ) -> Self {
        let initial_config = config_ref.read().unwrap().clone();
        let halflife_ticks =
            (initial_config.peak_decay_halflife_ms.max(10) as f32) / (MILLIS_PER_FRAME as f32);
        let ema_peak_decay_alpha = 0.5f32.powf(1.0 / halflife_ticks);
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
            ema_jitter: 0.0,
            ema_peak: 0.0,
            starvation_bump: 0.0,
            last_ingest_seq: None,
            startup_flush_remaining: 0,
            config: initial_config,
            config_ref,
            is_tcp_mode,
            hann_window: Self::make_hann_window(),
            ema_peak_decay_alpha,
            last_macro_spike: None,
            unstable_regime_until: None,
        }
    }

    /// Ceiling-division min depth (fixes C2: floor division bug).
    fn min_depth_frames(&self) -> u32 {
        Self::ms_to_frames_ceil(self.config.min_depth_ms)
    }

    fn comfort_cap_frames(&self) -> f32 {
        Self::ms_to_frames_ceil(self.config.comfort_cap_ms) as f32
    }

    /// Pure computation of the target buffer depth from observed jitter statistics.
    fn compute_target_depth(&self, tcp_cap_override: Option<f32>) -> u32 {
        // Static mode: lock buffer to exact user-specified depth, bypass all adaptive math.
        if let Some(static_ms) = self.config.static_target_ms {
            return Self::ms_to_frames_ceil(static_ms).max(self.min_depth_frames());
        }

        let jitter_margin = self.ema_jitter * 2.0 + self.ema_peak;
        // Target is natively built on top of the user's requested minimum floor.
        // We do not add artificial hardcoded safety margins here.
        let target = self.min_depth_frames() as f32 + jitter_margin + self.starvation_bump;
        let cap = tcp_cap_override.unwrap_or(self.comfort_cap_frames());
        let safe_cap = cap.max(self.min_depth_frames() as f32);
        
        target
            .ceil()
            .clamp(self.min_depth_frames() as f32, safe_cap) as u32
    }

    /// Drain all pending raw packets from the SPSC channel into the jitter buffer.
    /// Updates Dual-EMA jitter statistics from observed inter-arrival times.
    pub fn ingest_packets(&mut self, consumer: &mut HeapCons<RawPacket>) {
        while let Some(pkt) = consumer.try_pop() {
            if let Some(last_time) = self.last_network_arrival
                && let Some(last_seq) = self.last_ingest_seq
                // Only compute for forward progress. Ignore reordered packets for jitter math.
                && pkt.seq_num > last_seq
            {
                let seq_diff = pkt.seq_num - last_seq;

                // If the gap is impossibly large (> 5 seconds), it's likely a complete stream resume.
                // We don't want to record 5000ms of jitter. Discard extreme anomalies.
                if seq_diff < 1000 {
                    let iat_actual = pkt.arrival_time.duration_since(last_time).as_millis() as f32;
                    let iat_expected = (seq_diff as f32) * (MILLIS_PER_FRAME as f32);
                    let jitter_ms = (iat_actual - iat_expected).max(0.0);
                    let jitter_frames = jitter_ms / MILLIS_PER_FRAME as f32;

                    // Asymmetric EMA: fast attack (α=0.15) for sudden deterioration,
                    // slow decay (α=0.005) to prevent over-shedding during brief clean windows.
                    let alpha = if jitter_frames > self.ema_jitter {
                        0.15 // Fast attack
                    } else {
                        0.001 // Slow decay
                    };
                    self.ema_jitter = self.ema_jitter * (1.0 - alpha) + jitter_frames * alpha;

                    // Adjusts the peak decay speed on the fly based on repetitive spike density.
                    let mut current_decay_alpha = self.ema_peak_decay_alpha;

                    if self.config.peak_decay_halflife_ms == 0 {
                        // Smart Mode (Auto)
                        let mut is_unstable = false;
                        if let Some(unstable_until) = self.unstable_regime_until
                            && pkt.arrival_time < unstable_until
                        {
                            is_unstable = true;
                        }

                        // 34.6s half-life for chaotic Wi-Fi, 3.5s for clean Ethernet/5GHz
                        let halflife_ms = if is_unstable { 34600.0 } else { 3500.0 };
                        let halflife_ticks = halflife_ms / (MILLIS_PER_FRAME as f32);
                        current_decay_alpha = 0.5f32.powf(1.0 / halflife_ticks);

                        // Track spikes > 50ms (10 frames)
                        if jitter_frames >= 10.0 {
                            let mut is_new_macro_spike = false;
                            if let Some(last_spike) = self.last_macro_spike {
                                let interval =
                                    pkt.arrival_time.duration_since(last_spike).as_millis();
                                if interval > 500 {
                                    // Debounce burst packets
                                    is_new_macro_spike = true;
                                    // If spikes are frequent (<25s), network is chronically poor
                                    if interval < 25000 {
                                        self.unstable_regime_until = Some(
                                            pkt.arrival_time + std::time::Duration::from_secs(60),
                                        );
                                    }
                                }
                            } else {
                                is_new_macro_spike = true;
                            }

                            if is_new_macro_spike {
                                self.last_macro_spike = Some(pkt.arrival_time);
                            }
                        }
                    }

                    // ema_peak: slow-decay peak tracker.
                    // Jumps instantly on spikes, decays based on smart or preset half-life.
                    self.ema_peak = (self.ema_peak * current_decay_alpha).max(jitter_frames);
                }
            }
            self.last_network_arrival = Some(pkt.arrival_time);
            self.last_ingest_seq = Some(pkt.seq_num);
            self.buffer.insert(pkt);
        }
    }

    /// Fill `output` with PCM samples.
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
        // Slow bleed for starvation bump
        self.starvation_bump = (self.starvation_bump - 0.01).max(0.0);

        let mut pending_flush: Option<u32> = None;
        if let Ok(guard) = self.config_ref.try_read() {
            let new_config = guard.clone();
            if new_config != self.config {
                
                self.is_prebuffering = true;
                // Reset jitter tracking for clean convergence.
                self.ema_jitter = 0.0;
                self.ema_peak = 0.0;
                self.starvation_bump = 0.0;

                let new_target = Self::ms_to_frames_ceil(new_config.min_depth_ms).max(2);
                let flush_target = new_target + new_target / 2;
                if self.buffer.occupied_count() > flush_target {
                    pending_flush = Some(flush_target);
                }

                let halflife_ticks =
                    (new_config.peak_decay_halflife_ms.max(10) as f32) / (MILLIS_PER_FRAME as f32);
                self.ema_peak_decay_alpha = 0.5f32.powf(1.0 / halflife_ticks);

                self.config = new_config;
            }
        }

        if let Some(flush_target) = pending_flush {
            while self.buffer.occupied_count() > flush_target {
                if let Some(pkt) = self.buffer.pop_next() {
                    let _ = self.capture_pcm(&pkt);
                } else {
                    self.buffer.advance_one();
                }
            }
        }

        let min_depth = self.min_depth_frames();
        let tcp_mode = self.is_tcp_mode.load(Ordering::Relaxed);

        // USB/ADB multiplexing proxy naturally introduces transient OS locks and micro-jitter.
        let target = if tcp_mode {
            // Cap at 12 frames (60ms) to prevent overbuffering on USB.
            // If the user selected a low-latency preset like Wired, this also overrides their
            // native comfort cap (e.g. 4 frames) so ADB can safely absorb massive USB-transit batching.
            let dynamic = self.compute_target_depth(Some(12.0));

            // Allow user to overwrite natively if they chose Static
            if let Some(static_ms) = self.config.static_target_ms {
                Self::ms_to_frames_ceil(static_ms).max(self.min_depth_frames())
            } else {
                dynamic
            }
        } else {
            self.compute_target_depth(None)
        };

        // Soft ceiling = target + 80 frames. 
        // Only flush for truly catastrophic bloat (retains target + 30).
        let flush_ceiling = target + 80;
        if self.buffer.occupied_count() > flush_ceiling {
            let flush_to = target + 30;
            
            while self.buffer.occupied_count() > flush_to {
                if let Some(pkt) = self.buffer.pop_next() {
                    let _ = self.capture_pcm(&pkt);
                } else {
                    self.buffer.advance_one();
                }
            }
        }

        if self.is_prebuffering {
            let unpause_threshold =
                ((target as f32 * self.config.resume_threshold_pct) as u32).max(min_depth);
            if self.buffer.occupied_count() >= unpause_threshold {
                self.is_prebuffering = false;
                self.startup_flush_remaining = 100;
            } else {
                self.generate_plc();
                return;
            }
        }

        if self.startup_flush_remaining > 0 {
            self.startup_flush_remaining -= 1;
            let flush_to = target + 2;
            while self.buffer.occupied_count() > flush_to {
                if self.buffer.pop_next().is_none() {
                    self.buffer.advance_one();
                }
            }
            if self.startup_flush_remaining == 0 {
                let _ = self.decoder.reset_state();
                self.opus_next_expected_seq = None;
            }
        }

        if self.buffer.has_next() {
            self.gap_hold_count = 0;
            self.missing_count = 0;

            // Apply starvation bump if we just emerged from starvation.
            if self.starvation_count > 0 && !tcp_mode {
                let bump = (self.ema_peak * 2.0 + 4.0).min(60.0);
                self.starvation_bump = self.starvation_bump.max(bump);
                self.starvation_count = 0;
            }

            let pkt = self.buffer.pop_next().expect("has_next was true");
            let delay_ms = Instant::now().duration_since(pkt.arrival_time).as_millis() as u32;
            self.latency_metric.store(delay_ms, Ordering::Relaxed);
            let pcm = self.capture_pcm(&pkt);

            let occupied = self.buffer.occupied_count();
            if occupied > target + 2 && self.buffer.has_next() {
                let rms = Self::get_rms(&pcm);

                if rms < 0.005 {
                    // Silence fast-forward: append current frame (fix C1) AND pop an extra.
                    self.playback_buf.extend(&pcm);
                    if self.buffer.has_next() {
                        let extra = self.buffer.pop_next().unwrap();
                        let _ = self.capture_pcm(&extra);
                        // Extra frame is decoded (keeps Opus state) but discarded from output.
                    }
                    return;
                }

                // Perform Hann OLA WSOLA splice
                let pkt2 = self.buffer.pop_next().unwrap();
                let delay2_ms = Instant::now().duration_since(pkt2.arrival_time).as_millis() as u32;
                self.latency_metric.store(delay2_ms, Ordering::Relaxed);
                let pcm2 = self.capture_pcm(&pkt2);
                self.wsola_overlap_add(&pcm, &pcm2);
                return;
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

            if self.starvation_count >= 10 {
                self.is_prebuffering = true;
            }

            self.generate_plc();
            return;
        }

        if self.starvation_count > 0 && !tcp_mode {
            let bump = (self.ema_peak * 2.0 + 4.0).min(60.0);
            self.starvation_bump = self.starvation_bump.max(bump);
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

    /// Hann Overlap-Add WSOLA splice.
    ///
    /// Compresses time by finding the best phase-aligned splice point via 
    /// normalized cross-correlation and applying a Hann-windowed crossfade.
    fn wsola_overlap_add(&mut self, pcm1: &[f32], pcm2: &[f32]) {
        let ch = OPUS_CHANNELS as usize;
        let n = pcm1.len() / ch; // 240 sample-frames per packet

        // Guard: if packets are too small for OLA, just pass through
        if n < OLA_LEN + 16 {
            self.playback_buf.extend(pcm1);
            return;
        }

        let anchor = n - OLA_LEN; // 112

        // Find offset `d` in pcm2[0..SEARCH_RANGE] that maximizes correlation
        // with pcm1[anchor..anchor+OLA_LEN].
        let search_limit = SEARCH_RANGE.min(n.saturating_sub(OLA_LEN));
        let mut best_d = 0usize;
        let mut best_corr = f32::NEG_INFINITY;

        // Pre-compute energy of reference segment (pcm1 tail)
        let mut ref_energy = 0.0f32;
        for i in 0..OLA_LEN {
            for c in 0..ch {
                let idx = (anchor + i) * ch + c;
                if idx < pcm1.len() {
                    let s = pcm1[idx];
                    ref_energy += s * s;
                }
            }
        }

        for d in 0..search_limit {
            let mut cross = 0.0f32;
            let mut cand_energy = 0.0f32;
            for i in 0..OLA_LEN {
                for c in 0..ch {
                    let ref_idx = (anchor + i) * ch + c;
                    let cand_idx = (d + i) * ch + c;
                    let r = if ref_idx < pcm1.len() {
                        pcm1[ref_idx]
                    } else {
                        0.0
                    };
                    let s = if cand_idx < pcm2.len() {
                        pcm2[cand_idx]
                    } else {
                        0.0
                    };
                    cross += r * s;
                    cand_energy += s * s;
                }
            }
            // Normalized cross-correlation
            let denom = (ref_energy * cand_energy).sqrt();
            let ncc = if denom > 1e-10 { cross / denom } else { 0.0 };
            if ncc > best_corr {
                best_corr = ncc;
                best_d = d;
            }
        }

        // 1. pcm1[0..anchor] verbatim
        for f in 0..anchor {
            for c in 0..ch {
                let idx = f * ch + c;
                if idx < pcm1.len() {
                    self.playback_buf.push_back(pcm1[idx]);
                }
            }
        }

        // 2. Hann OLA crossfade
        for i in 0..OLA_LEN {
            let hann_out = 1.0 - self.hann_window[i]; // Fade-out: complement of fade-in
            let hann_in = self.hann_window[i]; // Fade-in: standard Hann
            for c in 0..ch {
                let ref_idx = (anchor + i) * ch + c;
                let cand_idx = (best_d + i) * ch + c;
                let r = if ref_idx < pcm1.len() {
                    pcm1[ref_idx]
                } else {
                    0.0
                };
                let s = if cand_idx < pcm2.len() {
                    pcm2[cand_idx]
                } else {
                    0.0
                };
                self.playback_buf.push_back(r * hann_out + s * hann_in);
            }
        }

        // 3. pcm2[best_d+OLA_LEN..n] verbatim
        for f in (best_d + OLA_LEN)..n {
            for c in 0..ch {
                let idx = f * ch + c;
                if idx < pcm2.len() {
                    self.playback_buf.push_back(pcm2[idx]);
                }
            }
        }
    }

    fn trigger_reset(&mut self) {
        self.buffer.reset();
        self.is_prebuffering = true;
        self.missing_count = 0;
        self.starvation_count = 0;
        self.gap_hold_count = 0;
        self.playback_buf.clear();
        self.decode_buf.fill(0.0);
        self.decode_len = 0;
        let _ = self.decoder.reset_state();
        self.ema_jitter = 0.0;
        self.ema_peak = 0.0;
        self.starvation_bump = 0.0;
        self.last_ingest_seq = None;
        self.startup_flush_remaining = 0;
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
        } else if self.decode_opus(&pkt.payload_data[..pkt.payload_len]) {
            self.decode_buf[..self.decode_len].to_vec()
        } else {
            self.decode_plc_to_buf();
            self.decode_buf[..self.decode_len].to_vec()
        };

        self.opus_next_expected_seq = Some(pkt.seq_num + 1);
        pcm
    }

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

    fn generate_plc(&mut self) {
        self.decode_plc_to_buf();
        self.playback_buf
            .extend(&self.decode_buf[..self.decode_len]);
        if let Some(expected) = self.opus_next_expected_seq {
            self.opus_next_expected_seq = Some(expected + 1);
        }
    }

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

    /// MIN_DEPTH = ceil(40ms / 5ms) = 8 frames.
    const MIN_DEPTH: u32 = 8;

    fn test_config() -> JitterConfig {
        JitterConfig {
            min_depth_ms: 40,
            comfort_cap_ms: 200,
            peak_decay_halflife_ms: 1000,
            resume_threshold_pct: 0.5,
            static_target_ms: None,
        }
    }

    fn setup_env() -> (
        JitterBufferManager,
        Encoder,
        ringbuf::HeapProd<RawPacket>,
        ringbuf::HeapCons<RawPacket>,
    ) {
        let decoder = Decoder::new(OPUS_SAMPLE_RATE, Channels::Stereo).unwrap();
        let encoder = Encoder::new(OPUS_SAMPLE_RATE, Channels::Stereo, Application::Audio).unwrap();
        let atomic = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let config_ref = Arc::new(std::sync::RwLock::new(test_config()));
        let is_tcp_mode = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let manager = JitterBufferManager::new(decoder, atomic, config_ref, is_tcp_mode);
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
            arrival_time: base_time + std::time::Duration::from_millis(seq * 5),
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
