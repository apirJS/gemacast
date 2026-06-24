use super::buffer::JitterBuffer;
use super::types::RawPacket;
use crate::audio::{OPUS_CHANNELS, OPUS_FRAME_SAMPLES, OPUS_FRAME_SIZE, OPUS_SAMPLE_RATE};
use crate::domain::types::JitterConfig;
use opus::Decoder;
use ringbuf::{HeapCons, traits::*};
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::RwLock;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Instant;

/// OLA window length in sample-frames for WSOLA crossfading.
/// 128 frames = 2.67ms at 48kHz — long enough for perceptual transparency.
const OLA_LEN: usize = 128;
/// Search range in sample-frames for cross-correlation alignment.
const SEARCH_RANGE: usize = 128;
const MILLIS_PER_FRAME: u32 = (OPUS_FRAME_SIZE as u32 * 1000) / OPUS_SAMPLE_RATE;
/// 2000ms max silence before resetting stream
const MAX_MISSING: u32 = 2000 / MILLIS_PER_FRAME;
/// Reorder tolerance: ~30ms window to wait for a reordered packet.
const REORDER_TOLERANCE: u32 = 30 / MILLIS_PER_FRAME;

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
    /// Precomputed smart-mode decay alpha for stable networks (3.5s half-life).
    smart_decay_stable: f32,
    /// Precomputed smart-mode decay alpha for unstable networks (34.6s half-life).
    smart_decay_unstable: f32,
    config: JitterConfig,
    config_ref: Arc<RwLock<JitterConfig>>,
    is_tcp_mode: Arc<AtomicBool>,
    /// Pre-computed Hann window for OLA crossfading (OLA_LEN entries).
    hann_window: Vec<f32>,
    /// Pre-allocated buffer for WSOLA: holds the first frame's PCM while decoding the second.
    wsola_buf: Vec<f32>,
    /// Countdown to reduce config lock polling: only check every 100 frames (~500ms).
    config_check_countdown: u32,
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
        let smart_decay_stable = 0.5f32.powf(1.0 / (3500.0 / MILLIS_PER_FRAME as f32));
        let smart_decay_unstable = 0.5f32.powf(1.0 / (34600.0 / MILLIS_PER_FRAME as f32));

        Self {
            decoder,
            buffer: JitterBuffer::new(),
            playback_buf: VecDeque::with_capacity(OPUS_FRAME_SAMPLES * 100),
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
            wsola_buf: vec![0.0f32; OPUS_FRAME_SAMPLES],
            config_check_countdown: 0,
            ema_peak_decay_alpha,
            last_macro_spike: None,
            unstable_regime_until: None,
            smart_decay_stable,
            smart_decay_unstable,
        }
    }

    /// Get the minimum buffer depth in frames.
    fn min_depth_frames(&self) -> u32 {
        Self::ms_to_frames_ceil(self.config.min_depth_ms)
    }

    /// Get the comfort cap in frames.
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
                    let iat_actual = match pkt.arrival_time.checked_duration_since(last_time) {
                        Some(d) => d.as_millis() as f32,
                        None => continue,
                    };
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
                        current_decay_alpha = if is_unstable {
                            self.smart_decay_unstable
                        } else {
                            self.smart_decay_stable
                        };
                        // Track spikes > 50ms (10 frames)
                        if jitter_frames >= 10.0 {
                            let mut is_new_macro_spike = false;
                            if let Some(last_spike) = self.last_macro_spike {
                                let interval =
                                    pkt.arrival_time.duration_since(last_spike).as_millis();
                                if interval > 500 {
                                    // Debounce burst packets
                                    is_new_macro_spike = true;
                                    // If spikes are frequent (<10s), network is chronically poor
                                    if interval < 10000 {
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

            use super::buffer::InsertResult;
            if matches!(self.buffer.insert(pkt), InsertResult::StreamRestarted) {
                let _ = self.decoder.reset_state();
                self.opus_next_expected_seq = None;
            }
        }
    }

    /// Fill `output` with PCM samples using bulk drain for SIMD-friendly access.
    pub fn fill_output(&mut self, output: &mut [f32], volume: f32) {
        let mut pos = 0;
        while pos < output.len() {
            if self.playback_buf.is_empty() {
                self.process_next_frame();
            }
            let need = output.len() - pos;
            let take = self.playback_buf.len().min(need);
            if take == 0 {
                output[pos..].fill(0.0);
                return;
            }
            // Bulk copy from VecDeque's contiguous slices for vectorization
            let (front, back) = self.playback_buf.as_slices();
            let from_front = take.min(front.len());
            for i in 0..from_front {
                output[pos + i] = front[i] * volume;
            }
            let from_back = take - from_front;
            for i in 0..from_back {
                output[pos + from_front + i] = back[i] * volume;
            }
            drop(self.playback_buf.drain(..take));
            pos += take;
        }
    }

    /// Process one Opus frame from the jitter buffer into the playback buffer.
    fn process_next_frame(&mut self) {
        // Fast bleed for starvation bump: 0.05 frames per callback (25ms per second bleed)
        self.starvation_bump = (self.starvation_bump - 0.05).max(0.0);
        let mut pending_flush: Option<u32> = None;
        self.config_check_countdown += 1;
        let should_check_config = self.config_check_countdown >= 100;
        if should_check_config {
            self.config_check_countdown = 0;
        }
        if should_check_config && let Ok(guard) = self.config_ref.try_read() {
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
                    self.capture_pcm(&pkt);
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

        let is_no_buffer = self.config.static_target_ms == Some(0);

        let flush_ceiling = if is_no_buffer {
            target + 3
        } else {
            target + 80
        };
        if self.buffer.occupied_count() > flush_ceiling {
            let flush_to = if is_no_buffer {
                target + 1
            } else {
                target + 30
            };
            if !is_no_buffer || self.buffer.occupied_count() > flush_to + 20 {
                let _ = self.decoder.reset_state();
                self.opus_next_expected_seq = None;
            }
            while self.buffer.occupied_count() > flush_to {
                if self.buffer.pop_next().is_none() {
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
            let flush_to = if is_no_buffer { target + 1 } else { target + 2 };
            while self.buffer.occupied_count() > flush_to {
                if let Some(pkt) = self.buffer.pop_next() {
                    self.capture_pcm(&pkt);
                } else {
                    self.buffer.advance_one();
                }
            }
        }
        if self.buffer.occupied_count() > 0 && !self.buffer.has_next() {
            self.gap_hold_count += 1;
            let mut fast_forward_seq = None;

            let tolerance = if is_no_buffer { 0 } else { REORDER_TOLERANCE };

            if let Some(lo) = self.buffer.lowest_available_seq() {
                let diff = lo.abs_diff(self.buffer.next_play_seq());
                if diff > 20 || self.gap_hold_count >= tolerance {
                    fast_forward_seq = Some(lo);
                }
            } else if self.gap_hold_count >= tolerance {
                self.buffer.advance_one();
                self.gap_hold_count = 0;
            }

            if let Some(lo) = fast_forward_seq {
                let diff = lo.saturating_sub(self.buffer.next_play_seq());
                self.buffer.fast_forward(lo);
                if diff > 20 {
                    let _ = self.decoder.reset_state();
                    self.opus_next_expected_seq = None;
                }
                self.gap_hold_count = 0;
            }
        }

        if self.buffer.has_next() {
            self.gap_hold_count = 0;
            self.missing_count = 0;

            // Apply starvation bump if we just emerged from starvation.
            if self.starvation_count > 0 && !tcp_mode {
                let bump = (self.ema_peak * 2.0 + 4.0).min(20.0);
                self.starvation_bump = self.starvation_bump.max(bump);
                self.starvation_count = 0;
            }

            let pkt = self.buffer.pop_next().expect("has_next was true");
            let delay_ms = Instant::now().duration_since(pkt.arrival_time).as_millis() as u32;
            self.latency_metric.store(delay_ms, Ordering::Relaxed);
            self.capture_pcm(&pkt);
            let occupied = self.buffer.occupied_count();
            let wsola_threshold = if is_no_buffer { target } else { target + 2 };
            if occupied > wsola_threshold && self.buffer.has_next() {
                let rms = Self::get_rms(&self.decode_buf[..self.decode_len]);
                if rms < 0.005 {
                    // Silence fast-forward: append current frame AND pop an extra.
                    self.playback_buf
                        .extend(&self.decode_buf[..self.decode_len]);
                    if self.buffer.has_next() {
                        let extra = self.buffer.pop_next().unwrap();
                        self.capture_pcm(&extra);
                        // Extra frame is decoded (keeps Opus state) but discarded from output.
                    }
                    return;
                }

                // WSOLA path: save current PCM, decode second frame
                let pcm1_len = self.decode_len;
                self.wsola_buf[..pcm1_len].copy_from_slice(&self.decode_buf[..pcm1_len]);
                let pkt2 = self.buffer.pop_next().unwrap();
                let delay2_ms = Instant::now().duration_since(pkt2.arrival_time).as_millis() as u32;

                self.latency_metric.store(delay2_ms, Ordering::Relaxed);
                self.capture_pcm(&pkt2);
                self.wsola_overlap_add_internal(pcm1_len);

                return;
            }
            self.playback_buf
                .extend(&self.decode_buf[..self.decode_len]);
            return;
        }

        self.missing_count += 1;

        if self.buffer.occupied_count() == 0 {
            self.gap_hold_count = 0;
            self.starvation_count += 1;
        }

        if self.missing_count > MAX_MISSING {
            self.trigger_reset();
            self.playback_buf
                .extend(std::iter::repeat_n(0.0, OPUS_FRAME_SAMPLES));
            return;
        }

        if self.buffer.occupied_count() == 0 {
            // Adaptive starvation threshold: on jittery UDP networks,
            // tolerate longer starvation before triggering a full rebuffer.
            // TCP/ADB is reliable, so keep the threshold tight.
            let starvation_threshold = if tcp_mode {
                10
            } else {
                let base = 10u32;
                base.saturating_add((self.ema_peak as u32).min(20)).min(40)
            };

            if self.starvation_count >= starvation_threshold {
                self.is_prebuffering = true;
            }
        }

        self.generate_plc();
    }

    /// Hann Overlap-Add WSOLA splice (allocation-free).
    ///
    /// Reads pcm1 from `self.wsola_buf[..pcm1_len]` and pcm2 from `self.decode_buf[..self.decode_len]`.
    /// Finds the best phase-aligned splice point via **mono-downmixed** normalized
    /// cross-correlation (halves FMA count vs full-stereo, enables NEON auto-vectorization),
    /// then applies a Hann-windowed crossfade on full stereo. Writes output to `self.playback_buf`.
    fn wsola_overlap_add_internal(&mut self, pcm1_len: usize) {
        let ch = OPUS_CHANNELS as usize;
        let pcm2_len = self.decode_len;
        let n1 = pcm1_len / ch;
        let n2 = pcm2_len / ch;

        // Guard: if packets are too small for OLA, just pass through pcm1
        if n1 < OLA_LEN + 16 || n2 < OLA_LEN + 16 {
            self.playback_buf.extend(&self.wsola_buf[..pcm1_len]);
            return;
        }

        let anchor = n1 - OLA_LEN;
        let search_limit = SEARCH_RANGE.min(n2.saturating_sub(OLA_LEN));
        // Mono-downmix optimization: pre-compute a contiguous mono reference
        // segment from the stereo tail of pcm1. Contiguous f32 layout enables
        // LLVM to auto-vectorize the inner correlation loop with NEON on ARM.
        let mut mono_ref = [0.0f32; OLA_LEN];
        let mut ref_energy = 0.0f32;
        for (i, m) in mono_ref.iter_mut().enumerate() {
            let base = (anchor + i) * ch;
            let mono = if ch == 2 {
                (self.wsola_buf[base] + self.wsola_buf[base + 1]) * 0.5
            } else {
                self.wsola_buf[base]
            };
            *m = mono;
            ref_energy += mono * mono;
        }

        let mut best_d = 0usize;
        let mut best_corr = f32::NEG_INFINITY;
        for d in 0..search_limit {
            let mut cross = 0.0f32;
            let mut cand_energy = 0.0f32;
            // Inner loop is now stride-1 on contiguous mono data — SIMD-friendly.
            for (i, &m) in mono_ref.iter().enumerate() {
                let base = (d + i) * ch;
                let mono_cand = if ch == 2 {
                    (self.decode_buf[base] + self.decode_buf[base + 1]) * 0.5
                } else {
                    self.decode_buf[base]
                };
                cross += m * mono_cand;
                cand_energy += mono_cand * mono_cand;
            }
            let denom = (ref_energy * cand_energy).sqrt();
            let ncc = if denom > 1e-10 { cross / denom } else { 0.0 };
            if ncc > best_corr {
                best_corr = ncc;
                best_d = d;
            }
        }

        // 1. pcm1[0..anchor] verbatim (bulk extend, no per-sample push)
        self.playback_buf.extend(&self.wsola_buf[..anchor * ch]);
        // 2. Hann OLA crossfade (full stereo for transparent output)
        for i in 0..OLA_LEN {
            let hann_in = self.hann_window[i];
            let hann_out = 1.0 - hann_in;
            for c in 0..ch {
                let r = self.wsola_buf[(anchor + i) * ch + c];
                let s = self.decode_buf[(best_d + i) * ch + c];
                self.playback_buf.push_back(r * hann_out + s * hann_in);
            }
        }

        // 3. pcm2[best_d+OLA_LEN..] verbatim (bulk extend)
        let tail_start = (best_d + OLA_LEN) * ch;
        if tail_start < pcm2_len {
            self.playback_buf
                .extend(&self.decode_buf[tail_start..pcm2_len]);
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

    /// Decode a packet's payload into `self.decode_buf[..self.decode_len]`.
    ///
    /// Zero-allocation: all output goes into the pre-allocated decode buffer.
    /// Silence frames output zeros without touching the decoder state.
    /// Uncompressed PCM frames are copied directly without decoder interaction.
    fn capture_pcm(&mut self, pkt: &RawPacket) {
        if let Some(expected) = self.opus_next_expected_seq
            && pkt.seq_num != expected
        {
            let gap = pkt.seq_num.saturating_sub(expected);
            if gap > 20 {
                // Large discontinuity (>100ms): full decoder reset.
                let _ = self.decoder.reset_state();
            } else if gap > 0 && gap <= 5 {
                // Small forward gap (5-25ms): feed PLC frames to keep decoder
                // state warm for smooth concealment. This prevents the hard
                // transient click that reset_state() would cause.
                for _ in 0..gap {
                    let _ = self.decoder.decode_float(&[], &mut self.decode_buf, false);
                }
            }
            // Gaps 6-20: decoder continues without intervention.
            // PLC quality degrades naturally but no hard reset click.
        }

        if pkt.is_silence {
            // Silence is intentional (sender detected quiet audio), not a loss
            // event. Don't feed PLC — it would poison the decoder's internal
            // state with hallucinated spectral data, causing a brief "warble"
            // artifact when real audio resumes.
            self.decode_buf[..OPUS_FRAME_SAMPLES].fill(0.0);
            self.decode_len = OPUS_FRAME_SAMPLES;
        } else if pkt.is_uncompressed {
            let f32_len = pkt.payload_len / std::mem::size_of::<f32>();
            if f32_len == 0 {
                // Empty uncompressed payload — generate PLC as fallback
                self.decode_plc_to_buf();
            } else {
                // Copy raw PCM directly without decoder interaction.
                // Don't feed PLC — uncompressed frames are a format choice,
                // not a loss event. Mixing PLC state into a non-Opus path
                // only poisons future Opus decode transitions.
                for (i, chunk) in pkt.payload_data[..pkt.payload_len]
                    .chunks_exact(4)
                    .enumerate()
                {
                    self.decode_buf[i] = f32::from_ne_bytes(chunk.try_into().unwrap());
                }
                self.decode_len = f32_len.min(self.decode_buf.len());
            }
        } else if !self.decode_opus(&pkt.payload_data[..pkt.payload_len]) {
            self.decode_plc_to_buf();
        }
        self.opus_next_expected_seq = Some(pkt.seq_num + 1);
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
        let mut pkt = RawPacket::zeroed();
        pkt.seq_num = seq;
        pkt.payload_data[..payload_len].copy_from_slice(&d);
        pkt.payload_len = payload_len;
        pkt.arrival_time = base_time + std::time::Duration::from_millis(seq * 5);
        pkt
    }

    #[test]
    fn should_output_silence_while_prebuffering_until_target_depth() {
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
    fn should_trigger_plc_and_recover_on_single_packet_loss() {
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
        assert_eq!(manager.missing_count, 0);
        assert!(!manager.is_prebuffering);
    }

    #[test]
    fn should_enter_prebuffering_after_sustained_starvation() {
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
    fn should_fast_forward_past_large_udp_sequence_gap() {
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
        assert_eq!(manager.missing_count, 0);
        assert!(!manager.is_prebuffering);
    }

    #[test]
    fn should_recover_from_three_second_network_drop() {
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
    fn should_reanchor_playhead_on_sender_crash_restart() {
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
    fn should_respect_static_target_ms_when_configured() {
        let decoder = Decoder::new(OPUS_SAMPLE_RATE, Channels::Stereo).unwrap();
        let atomic = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let static_config = JitterConfig {
            min_depth_ms: 10,
            comfort_cap_ms: 200,
            peak_decay_halflife_ms: 1000,
            resume_threshold_pct: 0.5,
            static_target_ms: Some(100), // Lock to 100ms = 20 frames
        };
        let config_ref = Arc::new(std::sync::RwLock::new(static_config));
        let is_tcp_mode = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut manager = JitterBufferManager::new(decoder, atomic, config_ref, is_tcp_mode);

        // Static mode should lock target to ceil(100ms / 5ms) = 20 frames
        let target = manager.compute_target_depth(None);
        assert_eq!(
            target, 20,
            "Static target should be exactly 20 frames for 100ms"
        );

        // Even with massive jitter, static target should not change
        manager.ema_jitter = 50.0;
        manager.ema_peak = 100.0;
        let target_after_jitter = manager.compute_target_depth(None);
        assert_eq!(
            target_after_jitter, 20,
            "Static target should ignore jitter"
        );
    }

    #[test]
    fn should_apply_volume_scaling_during_fill_output() {
        let (mut manager, mut encoder, mut prod, mut cons) = setup_env();
        let base_time = Instant::now();

        let make_noisy_packet =
            |encoder: &mut Encoder, seq: u64, base_time: Instant| -> RawPacket {
                let mut pcm = vec![0.0f32; OPUS_FRAME_SAMPLES];
                for (i, sample) in pcm.iter_mut().enumerate() {
                    *sample = if i % 2 == 0 { 0.5 } else { -0.5 };
                }
                let d = encoder.encode_vec_float(&pcm, 1500).unwrap();
                let payload_len = d.len();
                let mut pkt = RawPacket::zeroed();
                pkt.seq_num = seq;
                pkt.payload_data[..payload_len].copy_from_slice(&d);
                pkt.payload_len = payload_len;
                pkt.arrival_time = base_time + std::time::Duration::from_millis(seq * 5);
                pkt
            };

        // Fill enough to exit prebuffering.
        for i in 1..=MIN_DEPTH {
            assert!(
                prod.try_push(make_noisy_packet(&mut encoder, i as u64, base_time))
                    .is_ok()
            );
        }
        manager.ingest_packets(&mut cons);

        // Get output at full volume
        let mut full_vol = vec![0.0; OPUS_FRAME_SAMPLES];
        manager.fill_output(&mut full_vol, 1.0);

        // Reset and replay at half volume
        let (mut manager2, mut encoder2, mut prod2, mut cons2) = setup_env();
        for i in 1..=MIN_DEPTH {
            assert!(
                prod2
                    .try_push(make_noisy_packet(&mut encoder2, i as u64, base_time))
                    .is_ok()
            );
        }
        manager2.ingest_packets(&mut cons2);

        let mut half_vol = vec![0.0; OPUS_FRAME_SAMPLES];
        manager2.fill_output(&mut half_vol, 0.5);

        // Every non-zero sample at half volume should be ~half of full volume
        let mut checked = false;
        for (f, h) in full_vol.iter().zip(half_vol.iter()) {
            if f.abs() > 0.001 {
                let ratio = h / f;
                assert!(
                    (ratio - 0.5).abs() < 0.01,
                    "Expected half-volume ratio ~0.5, got {ratio}"
                );
                checked = true;
            }
        }
        assert!(
            checked,
            "Expected at least one non-zero sample to verify volume scaling"
        );
    }

    #[test]
    fn should_fast_forward_without_decoder_reset_on_small_gaps() {
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
        // Simulate a small 2 packet UDP loss.
        // We inject sequence (MIN_DEPTH + 3) into the buffer,
        // playhead expects (MIN_DEPTH + 1).
        let future_seq = MIN_DEPTH as u64 + 3;
        assert!(
            prod.try_push(make_packet(&mut encoder, future_seq, base_time))
                .is_ok()
        );
        manager.ingest_packets(&mut cons);

        // Wait for REORDER_TOLERANCE calls so gap_hold_count trips
        for _ in 0..REORDER_TOLERANCE {
            manager.fill_output(&mut output, 1.0);
        }

        // Now it should have fast-forwarded AND played the packet, so next_play_seq is future_seq + 1.
        assert_eq!(manager.buffer.next_play_seq(), future_seq + 1);
        // And the decoder state MUST be preserved (opus_next_expected_seq should not be None).
        // Since we waited REORDER_TOLERANCE frames, opus_next_expected_seq advanced via PLC!
        assert!(manager.opus_next_expected_seq.is_some());
    }

    #[test]
    fn should_aggressively_flush_in_no_buffer_mode_without_starvation() {
        let (mut manager, mut encoder, mut prod, mut cons) = setup_env();

        // Enable No Buffer mode
        let mut no_buffer_cfg = test_config();
        no_buffer_cfg.static_target_ms = Some(0);
        no_buffer_cfg.min_depth_ms = 0; // The UI enforces this for No Buffer
        {
            let mut w = manager.config_ref.write().unwrap();
            *w = no_buffer_cfg;
        }
        // Force the config update tick
        manager.config_check_countdown = 100;

        let base_time = Instant::now();

        // Inject a 10 packet burst!
        for i in 1..=10 {
            assert!(
                prod.try_push(make_packet(&mut encoder, i as u64, base_time))
                    .is_ok()
            );
        }
        manager.ingest_packets(&mut cons);

        let mut output = vec![0.0; OPUS_FRAME_SAMPLES];
        manager.fill_output(&mut output, 1.0);

        // In No Buffer mode, target is 0. flush_ceiling is 3. We had 10 packets.
        // It should flush down to flush_to = 1 packet, then decode that 1 packet.
        // After fill_output, occupied_count should be exactly 0 (it was 1, then got popped and played!).
        assert_eq!(manager.buffer.occupied_count(), 0);

        // It should not have starved, because it played the 1 packet.
        assert_eq!(manager.starvation_count, 0);
    }
}
