use super::buffer::JitterBuffer;
use super::stats::JitterStats;
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
/// 720 frames = 15ms at 48kHz, covering the full human pitch period range.
const SEARCH_RANGE: usize = 720;
const MILLIS_PER_FRAME: u32 = (OPUS_FRAME_SIZE as u32 * 1000) / OPUS_SAMPLE_RATE;
/// 2000ms max silence before resetting stream
const MAX_MISSING: u32 = 2000 / MILLIS_PER_FRAME;
/// Reorder tolerance: ~30ms window to wait for a reordered packet.
const REORDER_TOLERANCE: u32 = 30 / MILLIS_PER_FRAME;

/// Hysteresis half-width in frames. The effective target only moves when the
/// raw computed target deviates by more than this many frames.
const HYSTERESIS_BAND: u32 = 3;
/// How many consecutive callbacks the raw target must stay outside the
/// hysteresis band before we commit to the new effective target.
/// Now adaptive via `adaptive_dwell()` — this base value (40) is used for
/// low-cap presets only. See `adaptive_dwell()` for the full policy.
const _HYSTERESIS_DWELL_BASE: u32 = 40;
/// Snap effective target to multiples of this many frames to reduce
/// the total number of discrete target transitions.
const TARGET_QUANTUM: u32 = 4;
/// Cooldown period in callbacks after a starvation bump. While active,
/// no new bumps are applied — prevents positive-feedback ratcheting.
/// Reduced from 400 (2s) to 200 (1s): the probe_floor mechanism now
/// prevents the ratcheting that this cooldown was originally designed for.
const STARVATION_COOLDOWN: u32 = 200;
/// Rate-limit interval: effective target moves by at most ±1 frame every
/// this many callbacks, smoothing transitions for artifact-free playback.
const RAMP_INTERVAL: u32 = 5;
/// Minimum interval between timescale operations (in callbacks).
/// Prevents rapid-fire acceleration/expansion that causes audible artifacts.
/// 6 callbacks × 10ms/frame = 60ms, slightly above NetEQ's 50ms
/// (kMinTimescaleInterval=5 at 10ms frames). Each acceleration removes
/// ~3-10ms, so maximum drain rate is ~50-170ms/s.
/// In fast mode (≥3× target), cooldown is skipped entirely.
const MIN_TIMESCALE_INTERVAL: u32 = 6;
/// When the network has been stable for a sustained period, try probing
/// lower every this many callbacks. One quantum step down per probe.
const PROBE_DOWN_INTERVAL: u32 = 200;

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
    /// Stamping point for true NIC->DAC millisecond latency. Shared with receiver backend.
    latency_metric: Arc<AtomicU32>,
    /// How many consecutive callbacks we've been waiting for the current gap slot.
    /// Prevents spurious PLC for late-arriving reordered packets on 2.4GHz.
    gap_hold_count: u32,
    /// Additive target bump after starvation, bleeds continuously.
    starvation_bump: f32,
    /// Countdown for continuous startup flush.
    startup_flush_remaining: u32,
    config: JitterConfig,
    config_ref: Arc<RwLock<JitterConfig>>,
    is_tcp_mode: Arc<AtomicBool>,
    /// Pre-computed Hann window for OLA crossfading (OLA_LEN entries).
    hann_window: Vec<f32>,
    /// Pre-allocated buffer for WSOLA: holds the first frame's PCM while decoding the second.
    wsola_buf: Vec<f32>,
    /// Countdown to reduce config lock polling: only check every 100 frames (~500ms).
    config_check_countdown: u32,

    // --- Hysteresis & rate-limiting state (Fix 1, 2, 6) ---
    /// The currently locked-in effective target depth (frames). Only moves when
    /// the raw computed target exits the hysteresis band for HYSTERESIS_DWELL callbacks.
    effective_target: u32,
    /// How many consecutive callbacks the raw target has been outside the band.
    target_exit_count: u32,
    /// The quantized goal that `effective_target` is ramping toward.
    ramp_goal: u32,
    /// Countdown for rate-limited ramping (one step per RAMP_INTERVAL callbacks).
    ramp_countdown: u32,

    // --- Starvation bump cooldown (Fix 4) ---
    /// Cooldown countdown after a starvation bump. While >0, no new bumps are applied.
    starvation_bump_cooldown: u32,
    /// Countdown for active downward probing when the network is stable.
    probe_down_countdown: u32,
    /// Learned floor: the lowest effective_target that caused starvation.
    /// Probing won't go below this. Reset when network conditions genuinely change.
    probe_floor: u32,
    /// NetEQ-style IIR filtered buffer level to ignore instantaneous OS batching spikes.
    filtered_buffer_level: f32,
    /// Rolling network-condition statistics (jitter EMAs, clean streak, peak detection).
    stats: JitterStats,
    /// Cooldown counter for timescale operations (acceleration/expansion).
    /// While > 0, no new acceleration is attempted. Prevents rapid-fire
    /// time-stretching that causes audible artifacts on music.
    timescale_cooldown: u32,
    /// NetEQ-style starvation recovery guard. After the buffer drains to
    /// near-zero (starvation), suppress ALL acceleration for this many
    /// callbacks to let the buffer refill. Prevents the drain→starve→
    /// refill→drain saw-tooth cycle. Matches `prev_mode != kModeExpand`
    /// guard in WebRTC's decision_logic.cc:278.
    starvation_recovery: u32,
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
        let stats = JitterStats::new(&initial_config);

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
            latency_metric,
            gap_hold_count: 0,
            starvation_bump: 0.0,
            startup_flush_remaining: 0,
            config: initial_config,
            config_ref,
            is_tcp_mode,
            hann_window: Self::make_hann_window(),
            wsola_buf: vec![0.0f32; OPUS_FRAME_SAMPLES],
            config_check_countdown: 0,
            effective_target: 2,
            target_exit_count: 0,
            ramp_goal: 2,
            ramp_countdown: 0,
            starvation_bump_cooldown: 0,
            probe_down_countdown: PROBE_DOWN_INTERVAL,
            probe_floor: 0,
            filtered_buffer_level: 0.0,
            stats,
            timescale_cooldown: 0,
            starvation_recovery: 0,
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

    /// Compute the stability ratio from the clean streak counter.
    /// Returns 0.0 (unstable) to 1.0 (highly stable).
    /// Delegates to the [`JitterStats`] actor.
    fn stability_ratio(&self) -> f32 {
        self.stats.stability_ratio()
    }

    /// Snap a raw target to the nearest quantum level.
    /// Reduces the total number of discrete target transitions.
    fn quantize_target(raw: u32, quantum: u32) -> u32 {
        if quantum == 0 {
            return raw;
        }
        ((raw + quantum / 2) / quantum) * quantum
    }

    /// Adaptive quantum: when comfort_cap is small (≤ 8 frames / 80ms),
    /// use fine-grained quantum=1. High-cap presets (Auto, Resilient) use
    /// quantum=2 (20ms steps) for precise settling — quantum=4 caused
    /// probe overshoot (80ms→40ms skip over the 60ms floor).
    fn adaptive_quantum(&self) -> u32 {
        let cap = self.comfort_cap_frames() as u32;
        if cap <= 8 {
            1
        } else if cap >= 100 {
            2
        }
        // Auto/Resilient: 20ms steps
        else {
            TARGET_QUANTUM
        } // Balanced/Stable: 40ms steps
    }

    /// Adaptive hysteresis: narrow band for low-cap presets so the
    /// target can distinguish between 1, 2, 3 frames instead of treating
    /// them all as within the dead-zone.
    fn adaptive_hysteresis(&self) -> u32 {
        let cap = self.comfort_cap_frames() as u32;
        if cap <= 8 { 1 } else { HYSTERESIS_BAND }
    }

    /// Adaptive dwell time: how many callbacks the raw target must stay
    /// outside the hysteresis band before committing to a new goal.
    /// High-cap presets (Auto, Resilient) react faster since they have
    /// more headroom. Low-cap presets stay conservative.
    fn adaptive_dwell(&self) -> u32 {
        let cap = self.comfort_cap_frames() as u32;
        if cap <= 8 {
            40
        }
        // Low-cap: keep conservative
        else if cap >= 100 {
            15
        }
        // High-cap (Auto/Resilient): react faster
        else {
            25
        } // Mid-cap (Balanced/Stable)
    }

    /// Pure computation of the target buffer depth from observed jitter statistics.
    fn compute_target_depth(&self, tcp_cap_override: Option<f32>) -> u32 {
        // Static mode: lock buffer to exact user-specified depth, bypass all adaptive math.
        if let Some(static_ms) = self.config.static_target_ms {
            return Self::ms_to_frames_ceil(static_ms).max(self.min_depth_frames());
        }
        // When the network is demonstrably stable (high clean_streak), reduce the
        // jitter_margin contribution so the target converges to min_depth faster.
        let stability = self.stability_ratio();
        let margin_scale = 1.0 - stability * 0.4; // At full stability: 60% of raw margin
        let jitter_margin = (self.stats.ema_jitter * 2.0 + self.stats.ema_peak) * margin_scale;
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
            // Update jitter statistics from this arrival. Returns false to drop the
            // packet entirely (clock ran backwards vs. the last forward arrival).
            if !self.stats.observe(
                pkt.seq_num,
                pkt.arrival_time,
                &self.config,
                self.effective_target,
            ) {
                continue;
            }

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
        // --- NetEQ IIR Buffer Filter (Method 5) ---
        // Alpha = 254/256 ≈ 0.9921875. Heavily low-passes the instantaneous buffer level
        // so that massive batching (e.g. 10 packets arriving at once via USB) doesn't
        // trigger an instantaneous flush.
        let alpha = 254.0 / 256.0;
        self.filtered_buffer_level = self.filtered_buffer_level * alpha
            + (self.buffer.occupied_count() as f32) * (1.0 - alpha);

        // Proportional bleed for starvation bump: bigger bumps recover faster.
        // Increased rate from 0.05+3% to 0.08+5% — recovers ~40% faster.
        // 8-frame bump: bleeds at ~0.48 frames/cb → recovers in ~17 callbacks (85ms)
        // 2-frame bump: bleeds at ~0.18 frames/cb → recovers in ~11 callbacks (55ms)
        let bleed = 0.08 + self.starvation_bump * 0.05;
        self.starvation_bump = (self.starvation_bump - bleed).max(0.0);
        // Tick starvation bump cooldown.
        self.starvation_bump_cooldown = self.starvation_bump_cooldown.saturating_sub(1);
        self.timescale_cooldown = self.timescale_cooldown.saturating_sub(1);
        self.starvation_recovery = self.starvation_recovery.saturating_sub(1);
        let mut pending_flush: Option<u32> = None;
        self.config_check_countdown += 1;
        let should_check_config = self.config_check_countdown >= 100;
        if should_check_config {
            self.config_check_countdown = 0;
        }
        if should_check_config && let Ok(guard) = self.config_ref.try_read() {
            let new_config = guard.clone();
            if new_config != self.config {
                tracing::info!(
                    "[JitterMgr] Config changed: min_depth={}ms→{}ms, comfort_cap={}ms→{}ms, static={:?}→{:?}",
                    self.config.min_depth_ms,
                    new_config.min_depth_ms,
                    self.config.comfort_cap_ms,
                    new_config.comfort_cap_ms,
                    self.config.static_target_ms,
                    new_config.static_target_ms,
                );
                self.is_prebuffering = true;
                // Reset jitter tracking for clean convergence.
                self.stats.reset_on_config_change();
                self.starvation_bump = 0.0;
                // Reset hysteresis + ramp state for the new config.
                self.effective_target = Self::ms_to_frames_ceil(new_config.min_depth_ms).max(2);
                self.ramp_goal = self.effective_target;
                self.target_exit_count = 0;
                self.ramp_countdown = 0;
                self.starvation_bump_cooldown = 0;
                self.probe_down_countdown = PROBE_DOWN_INTERVAL;
                self.probe_floor = 0;
                self.filtered_buffer_level = 0.0;
                let new_target = self.effective_target;
                let flush_target = new_target + new_target / 2;
                if self.buffer.occupied_count() > flush_target {
                    pending_flush = Some(flush_target);
                }
                self.stats.recompute_decay_alpha(&new_config);
                self.config = new_config;
            }
        }
        if let Some(flush_target) = pending_flush {
            self.flush_with_crossfade(flush_target);
        }

        let min_depth = self.min_depth_frames();
        let tcp_mode = self.is_tcp_mode.load(Ordering::Relaxed);
        // USB/ADB multiplexing proxy naturally introduces transient OS locks and micro-jitter.
        let raw_target = if tcp_mode {
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

        // --- Hysteresis + quantization + rate-limited ramping ---
        // Static and No Buffer modes bypass hysteresis entirely.
        let target = if self.config.static_target_ms.is_some() {
            self.effective_target = raw_target;
            self.ramp_goal = raw_target;
            raw_target
        } else {
            let quantum = self.adaptive_quantum();
            let hysteresis = self.adaptive_hysteresis();
            let quantized = Self::quantize_target(raw_target, quantum).max(min_depth);
            let diff = self.effective_target.abs_diff(quantized);

            if diff <= hysteresis {
                // Inside the dead-zone: no change, reset dwell counter.
                self.target_exit_count = 0;
            } else {
                self.target_exit_count += 1;
                if self.target_exit_count >= self.adaptive_dwell() {
                    // Sustained deviation — commit to new ramp goal.
                    tracing::debug!(
                        "[JitterMgr] Target transition: effective={}→ramp_goal={}, raw={}, ema_jitter={:.2}, ema_peak={:.2}, stability={:.2}",
                        self.effective_target,
                        quantized,
                        raw_target,
                        self.stats.ema_jitter,
                        self.stats.ema_peak,
                        self.stability_ratio(),
                    );
                    self.ramp_goal = quantized;
                    self.target_exit_count = 0;
                    // If the target is moving UP, conditions changed — reset probe floor
                    // so future probing can re-discover the new optimal depth.
                    if quantized > self.effective_target {
                        self.probe_floor = 0;
                    }
                }
            }

            // Rate-limited ramp toward the goal.
            // Asymmetric speed: downward is faster (safe — we have excess buffer),
            // upward is slower (safety-critical — need stability).
            if self.effective_target != self.ramp_goal {
                if self.ramp_countdown == 0 {
                    if self.effective_target < self.ramp_goal {
                        self.effective_target += 1;
                        self.ramp_countdown = RAMP_INTERVAL; // 25ms upward steps
                    } else {
                        self.effective_target -= 1;
                        self.ramp_countdown = RAMP_INTERVAL; // Symmetric: same speed both directions
                    }
                }
                self.ramp_countdown = self.ramp_countdown.saturating_sub(1);
            } else if self.stability_ratio() > 0.2
                && self.effective_target > min_depth
                && self.starvation_bump < 0.5
                && self.starvation_bump_cooldown == 0
                // Allow probing even during unstable regime if current stability
                // is locally high enough — the regime lock is a coarse heuristic
                // and the probe_floor prevents re-probing below safe levels.
                && !self.stats.unstable_regime_until()
                    .is_some_and(|until| Instant::now() < until && self.stability_ratio() < 0.5)
            {
                // Active downward probing: when the network has been calm for
                // a sustained period, nudge the target down to discover the
                // lowest stable depth. Speed scales with confidence.
                let probe_interval = if self.stability_ratio() > 0.8 {
                    60 // High confidence: probe every ~300ms
                } else {
                    120 // Normal: probe every ~600ms
                };
                self.probe_down_countdown = self.probe_down_countdown.saturating_sub(1);
                if self.probe_down_countdown == 0 {
                    self.probe_down_countdown = probe_interval;
                    let probe_goal = Self::quantize_target(
                        self.effective_target.saturating_sub(quantum),
                        quantum,
                    )
                    .max(min_depth)
                    .max(self.probe_floor);
                    if probe_goal < self.effective_target {
                        tracing::debug!(
                            "[JitterMgr] Probe down: effective={}→probe_goal={}, floor={}, stability={:.2}",
                            self.effective_target,
                            probe_goal,
                            self.probe_floor,
                            self.stability_ratio(),
                        );
                        self.ramp_goal = probe_goal;
                    }
                }
            }
            self.effective_target
        };

        // Emergency flush: only in no-buffer mode where latency is critical.
        // For normal mode, use a gentle ceiling at 5× target to prevent
        // unbounded buildup when acceleration alone can't keep up.
        if is_no_buffer {
            let flush_ceiling = target + 3;
            if self.filtered_buffer_level as u32 > flush_ceiling {
                self.flush_with_crossfade(target + 1);
            }
        } else {
            // Gentle flush: if buffer exceeds 5× target, crossfade down to
            // 3× target. This is far gentler than the old flush (which went
            // to target+1). Leaves plenty of headroom for smooth acceleration.
            let gentle_ceiling = target.saturating_mul(5);
            if self.filtered_buffer_level as u32 > gentle_ceiling {
                self.flush_with_crossfade(target.saturating_mul(3));
            }
        }

        if self.is_prebuffering {
            let unpause_threshold =
                ((target as f32 * self.config.resume_threshold_pct) as u32).max(min_depth);
            if self.buffer.occupied_count() >= unpause_threshold {
                tracing::info!(
                    "[JitterMgr] Prebuffer complete: occupied={}, threshold={}, target={}",
                    self.buffer.occupied_count(),
                    unpause_threshold,
                    target,
                );
                self.is_prebuffering = false;
                // No startup_flush — the fast acceleration tier will
                // gradually drain excess while maintaining phase continuity.
            } else {
                self.generate_plc();
                return;
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

            // Apply starvation bump if we just emerged from starvation,
            // but only if the cooldown has expired (prevents ratcheting).
            if self.starvation_count > 0 && !tcp_mode {
                // NetEQ guard: after starvation, suppress acceleration for
                // 50 callbacks (~500ms) to let the buffer refill safely.
                // This prevents the drain→starve→refill→drain saw-tooth.
                self.starvation_recovery = 50;

                if self.starvation_bump_cooldown == 0 {
                    // Differentiate probe-induced starvation from genuine network outage.
                    // If we were recently stable (probing), use a mild bump that just
                    // returns to the previous level. If genuinely unstable, use full bump.
                    let is_probe_failure = self.stability_ratio() > 0.3;
                    if is_probe_failure {
                        let quantum = self.adaptive_quantum();
                        // Minimal bump: just 1 frame. The probe_floor mechanism
                        // prevents re-probing below this level, so we don't need
                        // a large bump to stay safe.
                        let mild_bump = 1.0;
                        self.starvation_bump = self.starvation_bump.max(mild_bump);
                        // Set floor using the full dynamic formula, not just +1 quantum.
                        // This prevents repeated starvation on bad networks: if ema_peak
                        // is 20 frames (100ms), floor jumps to ~200ms immediately.
                        let dynamic_floor = self.compute_target_depth(None);
                        self.probe_floor = self
                            .probe_floor
                            .max(dynamic_floor)
                            .max(self.effective_target.saturating_add(quantum));
                    } else {
                        // Genuine starvation: use ema_jitter (stable estimate) instead
                        // of ema_peak (spike-driven, can be vastly inflated). Cap at 8.
                        let bump = (self.stats.ema_jitter * 2.0 + 2.0).min(8.0);
                        self.starvation_bump = self.starvation_bump.max(bump);
                    }
                    tracing::info!(
                        "[JitterMgr] Starvation bump: type={}, bump={:.1}, effective_target={}, ema_jitter={:.2}, ema_peak={:.2}, starvation_frames={}",
                        if is_probe_failure { "probe" } else { "genuine" },
                        self.starvation_bump,
                        self.effective_target,
                        self.stats.ema_jitter,
                        self.stats.ema_peak,
                        self.starvation_count,
                    );
                    self.starvation_bump_cooldown = STARVATION_COOLDOWN;
                    // Upward bump bypasses hysteresis dwell for immediate safety.
                    let boosted = Self::quantize_target(
                        self.compute_target_depth(None),
                        self.adaptive_quantum(),
                    )
                    .max(self.min_depth_frames());
                    if boosted > self.effective_target {
                        self.ramp_goal = boosted;
                        self.target_exit_count = 0;
                    }
                }
                self.starvation_count = 0;
            }

            let pkt = self.buffer.pop_next().expect("has_next was true");
            let delay_ms = Instant::now().duration_since(pkt.arrival_time).as_millis() as u32;
            self.latency_metric.store(delay_ms, Ordering::Relaxed);
            self.capture_pcm(&pkt);

            // Smooth PLC→real audio transition: when the first real packet
            // arrives after starvation, apply a 2ms linear fade-in to mask
            // the spectral discontinuity between Opus PLC prediction and
            // real decoded audio. 96 samples = 2ms at 48kHz.
            if self.starvation_count > 0 {
                let fade_len = 96.min(self.decode_len);
                for i in 0..fade_len {
                    let gain = i as f32 / fade_len as f32;
                    self.decode_buf[i] *= gain;
                }
            }
            let occupied = self.buffer.occupied_count();
            // Keep 2-frame tolerance above target before triggering acceleration.
            // target+1 was tried but caused too-frequent OLA crossfades (clicking)
            // because the 5ms frame granularity makes single-frame gaps too sensitive.
            let wsola_threshold = if is_no_buffer { target } else { target + 2 };
            // NetEQ guard: skip ALL acceleration during starvation recovery.
            // This matches WebRTC's `prev_mode != kModeExpand` check that
            // prevents the drain→starve→refill→drain saw-tooth cycle.
            if occupied > wsola_threshold && self.starvation_recovery == 0 {
                let rms = Self::get_rms(&self.decode_buf[..self.decode_len]);
                let is_passive = rms < 0.005;
                if is_passive && self.buffer.has_next() {
                    // Silence fast-forward: append current frame AND pop extra(s).
                    // Cap shedding so we never drain below the target.
                    self.playback_buf
                        .extend(&self.decode_buf[..self.decode_len]);
                    let excess = occupied.saturating_sub(wsola_threshold);
                    let shed_count = (excess / 2).clamp(1, 4);
                    for _ in 0..shed_count {
                        if self.buffer.occupied_count() > wsola_threshold && self.buffer.has_next()
                        {
                            let extra = self.buffer.pop_next().unwrap();
                            self.capture_pcm(&extra);
                        }
                    }
                    self.timescale_cooldown = MIN_TIMESCALE_INTERVAL;
                    return;
                }

                // NetEQ-style tiered acceleration:
                // Fast mode (buffer ≥ 3× target): NCC 0.5, no cooldown
                //   — emergency drain, always fires regardless of audio content.
                // Normal mode: NCC 0.9, with cooldown AND energy gate.
                //   Only accelerate during quiet passages (rms < 0.08) where
                //   crossfade artifacts are masked. During loud music, accept
                //   temporary buffer excess — it drains at the next quiet moment.
                let fast_threshold = target.saturating_mul(3);
                let is_fast = occupied > fast_threshold;
                let is_quiet_enough = rms < 0.08;
                if (is_fast || (self.timescale_cooldown == 0 && is_quiet_enough))
                    && self.try_accelerate_internal(is_fast)
                {
                    tracing::trace!(
                        "[JitterMgr] Accelerate: occupied={}, target={}, fast={}, rms={:.4}",
                        occupied,
                        target,
                        is_fast,
                        rms,
                    );
                    if !is_fast {
                        self.timescale_cooldown = MIN_TIMESCALE_INTERVAL;
                    }
                    return;
                }
            }

            // Note: trickle acceleration (drain when occupied is between target
            // and target+2) was removed — it caused audible clicking from
            // too-frequent OLA crossfades at 48kHz/10ms frame granularity.

            // --- Method 1: Preemptive Expand ---
            let min_depth = self.min_depth_frames();
            let is_low_buffer = self.filtered_buffer_level < min_depth as f32;
            if is_low_buffer {
                let rms = Self::get_rms(&self.decode_buf[..self.decode_len]);
                // Only stretch quiet audio — WSOLA expansion on loud music causes
                // audible "slowing down" artifacts. On loud active audio, let the
                // buffer briefly dip below min_depth; packets will refill naturally.
                // rms > 0.001 excludes true silence (nothing to stretch).
                if rms < 0.08 && rms > 0.001 && self.try_wsola_expand_internal() {
                    tracing::trace!(
                        "[JitterMgr] Expand: filtered_level={:.1}, min_depth={}, rms={:.4}",
                        self.filtered_buffer_level,
                        min_depth,
                        rms,
                    );
                    return;
                }
            }

            self.playback_buf
                .extend(&self.decode_buf[..self.decode_len]);
            return;
        }

        self.missing_count += 1;

        if self.buffer.occupied_count() == 0 {
            self.gap_hold_count = 0;
            self.starvation_count += 1;
            if self.starvation_count == 1 {
                tracing::warn!(
                    "[JitterMgr] Starvation started: effective_target={}, ema_jitter={:.2}, ema_peak={:.2}",
                    self.effective_target,
                    self.stats.ema_jitter,
                    self.stats.ema_peak,
                );
            }
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
                base.saturating_add((self.stats.ema_peak as u32).min(20))
                    .min(40)
            };

            if self.starvation_count >= starvation_threshold {
                tracing::warn!(
                    "[JitterMgr] Starvation→rebuffer: starvation_count={}, threshold={}",
                    self.starvation_count,
                    starvation_threshold,
                );
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
    fn try_wsola_overlap_add_internal(&mut self, pcm1_len: usize, force_crossfade: bool) -> bool {
        let ch = OPUS_CHANNELS as usize;
        let pcm2_len = self.decode_len;
        let n1 = pcm1_len / ch;
        let n2 = pcm2_len / ch;

        // Guard: if packets are too small for OLA, just pass through pcm1
        if n1 < OLA_LEN + 16 || n2 < OLA_LEN + 16 {
            return false;
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

        // --- NetEQ VAD & Gentle Acceleration (Method 2 & 4) ---
        // If active speech (not forced) and correlation is weak (< 0.9), abort!
        if !force_crossfade && best_corr < 0.9 {
            return false;
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

        true
    }

    /// NetEQ Preemptive Expand (Method 1).
    /// Stretches the current decode buffer by exactly one pitch period (up to 15ms)
    /// to slow down playback and prevent an imminent starvation gap.
    fn try_wsola_expand_internal(&mut self) -> bool {
        let ch = OPUS_CHANNELS as usize;
        let n = self.decode_len / ch;
        if n < OLA_LEN + 16 {
            return false;
        }

        let anchor = n - OLA_LEN;
        let search_limit = SEARCH_RANGE.min(anchor.saturating_sub(16));
        if search_limit == 0 {
            return false;
        }

        let mut mono_ref = [0.0f32; OLA_LEN];
        let mut ref_energy = 0.0f32;
        for (i, m) in mono_ref.iter_mut().enumerate() {
            let base = (anchor + i) * ch;
            let mono = if ch == 2 {
                (self.decode_buf[base] + self.decode_buf[base + 1]) * 0.5
            } else {
                self.decode_buf[base]
            };
            *m = mono;
            ref_energy += mono * mono;
        }

        let mut best_d = 0usize;
        let mut best_corr = f32::NEG_INFINITY;
        for d in 0..search_limit {
            let mut cross = 0.0f32;
            let mut cand_energy = 0.0f32;
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

        // NetEQ requires strong correlation (>0.9) to stretch, otherwise it causes robotic artifacts
        if best_corr < 0.9 {
            return false;
        }

        // 1. pcm[0..anchor] verbatim
        self.playback_buf.extend(&self.decode_buf[..anchor * ch]);
        // 2. Hann OLA crossfade
        for i in 0..OLA_LEN {
            let hann_in = self.hann_window[i];
            let hann_out = 1.0 - hann_in;
            for c in 0..ch {
                let r = self.decode_buf[(anchor + i) * ch + c];
                let s = self.decode_buf[(best_d + i) * ch + c];
                self.playback_buf.push_back(r * hann_out + s * hann_in);
            }
        }
        // 3. pcm[best_d+OLA_LEN..end] verbatim
        let tail_start = (best_d + OLA_LEN) * ch;
        if tail_start < self.decode_len {
            self.playback_buf
                .extend(&self.decode_buf[tail_start..self.decode_len]);
        }

        true
    }

    /// NetEQ-style single-frame acceleration.
    ///
    /// Finds the pitch period via **autocorrelation** within the current
    /// `decode_buf`, then removes one pitch period via Hann overlap-add.
    ///
    /// Key difference from the old cross-packet WSOLA: this correlates the
    /// signal **with itself** (autocorrelation), not two different packets.
    /// Autocorrelation on tonal audio (speech, music) almost always succeeds
    /// because periodic signals repeat themselves within a single frame.
    fn try_accelerate_internal(&mut self, fast_mode: bool) -> bool {
        let ch = OPUS_CHANNELS as usize;
        let n = self.decode_len / ch;
        // Need at least 2*OLA_LEN to have non-overlapping search + reference
        if n < 2 * OLA_LEN + 16 {
            return false;
        }

        // Reference: the TAIL of the frame (last OLA_LEN sample-frames)
        let anchor = n - OLA_LEN;
        // Search limit: search window must not overlap the reference window
        let search_limit = anchor.saturating_sub(OLA_LEN);
        if search_limit == 0 {
            return false;
        }

        // --- Step 1: Autocorrelation to find pitch period ---
        // Mono-downmix the tail for fast NCC (same technique as expand)
        let mut mono_ref = [0.0f32; OLA_LEN];
        let mut ref_energy = 0.0f32;
        for (i, m) in mono_ref.iter_mut().enumerate() {
            let base = (anchor + i) * ch;
            let mono = if ch == 2 {
                (self.decode_buf[base] + self.decode_buf[base + 1]) * 0.5
            } else {
                self.decode_buf[base]
            };
            *m = mono;
            ref_energy += mono * mono;
        }

        // Search WITHIN the same frame for the best matching segment
        let mut best_d = 0usize;
        let mut best_corr = f32::NEG_INFINITY;
        for d in 0..search_limit {
            let mut cross = 0.0f32;
            let mut cand_energy = 0.0f32;
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

        // NetEQ thresholds: 0.9 for normal, 0.5 for fast mode (kFastAccelerate).
        // Fast mode activates when buffer is extremely overfull — trades
        // slightly lower quality for much faster drain.
        let threshold = if fast_mode { 0.5 } else { 0.9 };
        if best_corr < threshold {
            return false;
        }

        // Pitch period = distance between matching section and reference
        let pitch_period = anchor - best_d;
        if pitch_period < OLA_LEN {
            return false;
        }

        // --- Step 2: Remove pitch period(s) via overlap-add ---
        // In fast mode, remove MULTIPLE pitch periods per operation.
        // This matches NetEQ accelerate.cc:62-67:
        //   peak_index = (fs_mult_120 / peak_index) * peak_index;
        // Removing more per-op means fewer OLA crossfades needed,
        // which eliminates the 'clicking' artifacts from too-frequent
        // phase discontinuities.
        let remove_len = if fast_mode {
            // Half the frame length in sample-frames, rounded to
            // multiple of pitch period. Cap at (anchor - best_d) to
            // stay within the frame.
            let half_frame = n / 2;
            let multiples = half_frame / pitch_period;
            let multi_remove = multiples.max(1) * pitch_period;
            multi_remove.min(anchor - best_d)
        } else {
            pitch_period
        };

        // Splice point after removing `remove_len` samples:
        // Output: [0..best_d] + crossfade([best_d..], [best_d+remove_len..]) + tail
        let splice_start = best_d + remove_len;
        if splice_start + OLA_LEN > n {
            return false; // Not enough room for crossfade
        }

        // 1. [0..best_d] verbatim
        self.playback_buf.extend(&self.decode_buf[..best_d * ch]);

        // 2. Hann OLA crossfade between the two pitch-aligned sections
        for i in 0..OLA_LEN {
            let hann_in = self.hann_window[i];
            let hann_out = 1.0 - hann_in;
            for c in 0..ch {
                let early = self.decode_buf[(best_d + i) * ch + c];
                let late = self.decode_buf[(splice_start + i) * ch + c];
                self.playback_buf
                    .push_back(early * hann_out + late * hann_in);
            }
        }

        // 3. Tail after the crossfade
        let tail_start = (splice_start + OLA_LEN) * ch;
        if tail_start < self.decode_len {
            self.playback_buf
                .extend(&self.decode_buf[tail_start..self.decode_len]);
        }

        true
    }

    /// Flush buffer down to `flush_to` frames with a WSOLA crossfade across
    /// the skip boundary. Keeps the decoder state warm by decoding every
    /// skipped packet (output is discarded), then splices the pre-flush and
    /// post-flush audio with the existing Hann OLA window.
    fn flush_with_crossfade(&mut self, flush_to: u32) {
        if self.buffer.occupied_count() <= flush_to {
            return;
        }
        tracing::info!(
            "[JitterMgr] Flush: occupied={}→target={}, effective_target={}",
            self.buffer.occupied_count(),
            flush_to,
            self.effective_target,
        );
        // 1. Snapshot the current decoded PCM into wsola_buf.
        let pre_flush_len = self.decode_len;
        if pre_flush_len > 0 {
            self.wsola_buf[..pre_flush_len].copy_from_slice(&self.decode_buf[..pre_flush_len]);
        }
        // 2. Skip frames, feeding each to the decoder to keep its state warm.
        //    This avoids the hard transient click that reset_state() causes.
        while self.buffer.occupied_count() > flush_to {
            if let Some(pkt) = self.buffer.pop_next() {
                self.capture_pcm(&pkt);
            } else {
                self.buffer.advance_one();
            }
        }
        // 3. Crossfade between pre-flush and post-flush audio.
        if pre_flush_len > 0
            && self.decode_len > 0
            && !self.try_wsola_overlap_add_internal(pre_flush_len, true)
        {
            self.playback_buf.extend(&self.wsola_buf[..pre_flush_len]);
            self.playback_buf
                .extend(&self.decode_buf[..self.decode_len]);
        }
    }

    fn trigger_reset(&mut self) {
        tracing::warn!(
            "[JitterMgr] Stream reset: missing_count exceeded {}ms silence threshold",
            MAX_MISSING * MILLIS_PER_FRAME,
        );
        self.buffer.reset();
        self.is_prebuffering = true;
        self.missing_count = 0;
        self.starvation_count = 0;
        self.gap_hold_count = 0;
        self.playback_buf.clear();
        self.decode_buf.fill(0.0);
        self.decode_len = 0;
        let _ = self.decoder.reset_state();
        self.stats.reset_on_stream_restart();
        self.starvation_bump = 0.0;
        self.startup_flush_remaining = 0;
        self.effective_target = 2;
        self.ramp_goal = 2;
        self.target_exit_count = 0;
        self.ramp_countdown = 0;
        self.starvation_bump_cooldown = 0;
        self.probe_down_countdown = PROBE_DOWN_INTERVAL;
        self.probe_floor = 0;
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

        // Gradually fade PLC output to silence over frames 4-7 of starvation.
        // Opus PLC quality degrades rapidly after ~3 frames (15ms). Beyond that,
        // the prediction sounds robotic — silence is less jarring than bad prediction.
        if self.starvation_count > 3 {
            let fade = (1.0 - ((self.starvation_count - 3) as f32 / 4.0)).max(0.0);
            for s in &mut self.decode_buf[..self.decode_len] {
                *s *= fade;
            }
        }

        self.playback_buf
            .extend(&self.decode_buf[..self.decode_len]);
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

    /// MIN_DEPTH = ceil(40ms / MILLIS_PER_FRAME)
    const MIN_DEPTH: u32 = 40 / MILLIS_PER_FRAME;
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
        pkt.arrival_time =
            base_time + std::time::Duration::from_millis(seq * MILLIS_PER_FRAME as u64);
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
            static_target_ms: Some(100), // Lock to 100ms
        };
        let config_ref = Arc::new(std::sync::RwLock::new(static_config));
        let is_tcp_mode = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut manager = JitterBufferManager::new(decoder, atomic, config_ref, is_tcp_mode);

        // Static mode should lock target to ceil(100ms / MILLIS_PER_FRAME)
        let expected = 100 / MILLIS_PER_FRAME;
        let target = manager.compute_target_depth(None);
        assert_eq!(
            target, expected,
            "Static target should be exactly {} frames for 100ms",
            expected
        );

        // Even with massive jitter, static target should not change
        manager.stats.ema_jitter = 50.0;
        manager.stats.ema_peak = 100.0;
        let target_after_jitter = manager.compute_target_depth(None);
        assert_eq!(
            target_after_jitter, expected,
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

        // In No Buffer mode, target is 0. The config update path transiently
        // sets effective_target = max(min_depth_frames, 2) = 2, causing an
        // initial flush to 3 packets. After the static target override kicks in
        // (target = 0), silence fast-forward drains most remaining packets.
        // At most 1 packet may remain after a single fill_output call — it will
        // be consumed on the next call. The key invariant: no starvation.
        assert!(
            manager.buffer.occupied_count() <= 1,
            "Expected at most 1 packet remaining after aggressive flush, got {}",
            manager.buffer.occupied_count()
        );

        // It should not have starved, because it played packets.
        assert_eq!(manager.starvation_count, 0);
    }

    #[test]
    fn clean_streak_should_build_on_stable_packets_and_reset_on_spikes() {
        let (mut manager, mut encoder, mut prod, mut cons) = setup_env();
        let base_time = Instant::now();

        // Feed 100 clean packets with perfect 5ms inter-arrival (zero jitter).
        for i in 1..=100 {
            assert!(
                prod.try_push(make_packet(&mut encoder, i as u64, base_time))
                    .is_ok()
            );
        }
        manager.ingest_packets(&mut cons);

        // clean_streak should be high (all packets had jitter < 1 frame)
        assert!(
            manager.stats.clean_streak >= 90,
            "Expected clean_streak >= 90 after 100 clean packets, got {}",
            manager.stats.clean_streak
        );

        // stability_ratio should be meaningfully positive
        assert!(
            manager.stability_ratio() > 0.2,
            "Expected stability_ratio > 0.2, got {}",
            manager.stability_ratio()
        );

        // Now inject a severe spike: a packet with 200ms delay (40 frames of jitter)
        let mut spike_pkt = make_packet(&mut encoder, 101, base_time);
        spike_pkt.arrival_time =
            base_time + std::time::Duration::from_millis(101 * MILLIS_PER_FRAME as u64 + 200);
        assert!(prod.try_push(spike_pkt).is_ok());
        manager.ingest_packets(&mut cons);

        // Severe spike (>4 frames) should slam clean_streak to 0
        assert_eq!(
            manager.stats.clean_streak, 0,
            "Expected clean_streak = 0 after severe spike"
        );
    }

    #[test]
    fn stable_network_should_decay_jitter_faster_than_unstable() {
        let (mut manager1, mut encoder1, mut prod1, mut cons1) = setup_env();
        let (mut manager2, mut encoder2, mut prod2, mut cons2) = setup_env();
        let base_time = Instant::now();

        // Both managers: inject a spike to raise ema_jitter
        let spike_offset = std::time::Duration::from_millis(100); // 100ms jitter
        for i in 1..=2 {
            let mut pkt1 = make_packet(&mut encoder1, i, base_time);
            let mut pkt2 = make_packet(&mut encoder2, i, base_time);
            if i == 2 {
                pkt1.arrival_time += spike_offset;
                pkt2.arrival_time += spike_offset;
            }
            assert!(prod1.try_push(pkt1).is_ok());
            assert!(prod2.try_push(pkt2).is_ok());
        }
        manager1.ingest_packets(&mut cons1);
        manager2.ingest_packets(&mut cons2);

        let spike_jitter1 = manager1.stats.ema_jitter;
        let spike_jitter2 = manager2.stats.ema_jitter;
        assert!(
            (spike_jitter1 - spike_jitter2).abs() < 0.01,
            "Both managers should have the same jitter after the spike"
        );

        // Manager 1: simulate a stable network (400 clean packets → full stability)
        manager1.stats.clean_streak = 400;
        // Manager 2: simulate an unstable network (0 clean streak)
        manager2.stats.clean_streak = 0;

        // Feed 200 clean packets to both (zero jitter)
        for i in 3..=202 {
            let pkt1 = make_packet(&mut encoder1, i, base_time);
            let pkt2 = make_packet(&mut encoder2, i, base_time);
            assert!(prod1.try_push(pkt1).is_ok());
            assert!(prod2.try_push(pkt2).is_ok());
        }
        manager1.ingest_packets(&mut cons1);
        manager2.ingest_packets(&mut cons2);

        // The stable manager should have decayed significantly faster
        assert!(
            manager1.stats.ema_jitter < manager2.stats.ema_jitter,
            "Stable network jitter ({}) should be less than unstable ({})",
            manager1.stats.ema_jitter,
            manager2.stats.ema_jitter
        );
    }

    #[test]
    fn proportional_bleed_should_recover_large_bumps_faster() {
        let (mut manager, _, _, _) = setup_env();

        // Simulate a large starvation bump
        manager.starvation_bump = 20.0;
        manager.is_prebuffering = false;

        // Record the bleed rate at 20.0
        let initial = manager.starvation_bump;
        let mut output = vec![0.0; OPUS_FRAME_SAMPLES];
        manager.fill_output(&mut output, 1.0);
        let after_one = manager.starvation_bump;
        let bleed_large = initial - after_one;

        // Now set a small bump and measure bleed rate
        manager.starvation_bump = 2.0;
        let initial_small = manager.starvation_bump;
        manager.fill_output(&mut output, 1.0);
        let after_one_small = manager.starvation_bump;
        let bleed_small = initial_small - after_one_small;

        // The large bump should bleed faster (proportional bleed)
        assert!(
            bleed_large > bleed_small * 2.0,
            "Large bump bleed ({bleed_large}) should be > 2x small bump bleed ({bleed_small})"
        );
    }
    #[test]
    fn hysteresis_should_ignore_transient_spikes() {
        let (mut manager, _, _, _) = setup_env();
        manager.is_prebuffering = false;

        // Set a known effective target and ramp goal.
        manager.effective_target = 12;
        manager.ramp_goal = 12;
        manager.target_exit_count = 0;

        // Simulate a single 100ms jitter spike.
        manager.stats.ema_jitter = 0.0;
        manager.stats.ema_peak = 0.0;
        manager.starvation_bump = 0.0;

        // Inject a spike that raises ema_jitter temporarily.
        manager.stats.ema_jitter = 10.0; // This would compute a high raw_target.
        manager.stats.ema_peak = 15.0;

        // Call process_next_frame once (no packets, will generate PLC).
        let mut output = vec![0.0; OPUS_FRAME_SAMPLES];
        manager.fill_output(&mut output, 1.0);

        // The effective_target must NOT have jumped to the spike-induced value.
        // Hysteresis requires HYSTERESIS_DWELL (40) consecutive callbacks outside the band.
        assert_eq!(
            manager.effective_target, 12,
            "Effective target should stay at 12 after a single spike-induced fill_output, got {}",
            manager.effective_target
        );
    }

    #[test]
    fn starvation_bump_cooldown_should_prevent_ratcheting() {
        let (mut manager, mut encoder, mut prod, mut cons) = setup_env();
        let base_time = Instant::now();
        let mut seq = 1u64;

        // Fill to exit prebuffering.
        for _ in 0..MIN_DEPTH {
            assert!(
                prod.try_push(make_packet(&mut encoder, seq, base_time))
                    .is_ok()
            );
            seq += 1;
        }
        manager.ingest_packets(&mut cons);
        let mut output = vec![0.0; OPUS_FRAME_SAMPLES];
        for _ in 0..MIN_DEPTH {
            manager.fill_output(&mut output, 1.0);
        }

        // Trigger first starvation (buffer drains completely).
        for _ in 0..15 {
            manager.fill_output(&mut output, 1.0);
        }
        assert!(manager.is_prebuffering);

        // Recover: push contiguous packets from where we left off.
        // Push extra packets to ensure has_next() fires after prebuffering exits.
        let recover_count = MIN_DEPTH + 4;
        for _ in 0..recover_count {
            assert!(
                prod.try_push(make_packet(&mut encoder, seq, base_time))
                    .is_ok()
            );
            seq += 1;
        }
        manager.ingest_packets(&mut cons);

        // Drain enough frames to exit prebuffering and let the bump apply.
        for _ in 0..4 {
            manager.fill_output(&mut output, 1.0);
        }
        assert!(!manager.is_prebuffering, "Should have exited prebuffering");
        let bump_after_first = manager.starvation_bump;
        assert!(
            bump_after_first > 0.0,
            "First starvation bump should have been applied, got {bump_after_first}"
        );

        // Drain remaining packets to trigger a second starvation.
        for _ in 0..20 {
            manager.fill_output(&mut output, 1.0);
        }

        // Recover again with contiguous packets.
        let recover2_count = MIN_DEPTH + 4;
        for _ in 0..recover2_count {
            assert!(
                prod.try_push(make_packet(&mut encoder, seq, base_time))
                    .is_ok()
            );
            seq += 1;
        }
        manager.ingest_packets(&mut cons);
        for _ in 0..4 {
            manager.fill_output(&mut output, 1.0);
        }

        // The second bump should NOT have been applied (cooldown still active).
        // starvation_bump should have only bled from the first bump, not re-applied.
        assert!(
            manager.starvation_bump < bump_after_first,
            "Second starvation within cooldown should NOT re-apply bump. \
             bump_after_first={bump_after_first}, current={}",
            manager.starvation_bump,
        );
    }

    #[test]
    fn regime_aware_clean_threshold_should_build_streak_on_2_4ghz() {
        let (mut manager, mut encoder, mut prod, mut cons) = setup_env();
        let base_time = Instant::now();

        // Simulate 2.4GHz-like jitter: consistent 20ms jitter per packet.
        // Each packet arrives at 30ms intervals instead of the expected 10ms.
        // IAT = 30ms, expected = 10ms, jitter = 20ms = 2 frames.
        //
        // With the old fixed threshold (< 1 frame = 5ms), clean_streak would never build.
        // With the adaptive threshold, ema_jitter settles around 2 frames,
        // clean_threshold ≈ 2*1.5+1.0 = 4.0 frames, and 2-frame jitter counts as "clean".

        for i in 1..=200u64 {
            let mut pkt = make_packet(&mut encoder, i, base_time);
            // 30ms spacing creates 20ms jitter (30ms actual - 10ms expected per frame).
            pkt.arrival_time = base_time + std::time::Duration::from_millis(i * 30);
            assert!(prod.try_push(pkt).is_ok());
        }
        manager.ingest_packets(&mut cons);

        // ema_jitter should have settled around 2 frames (20ms jitter / 10ms per frame).
        assert!(
            manager.stats.ema_jitter > 1.0,
            "Expected ema_jitter > 1.0 for 20ms baseline jitter, got {}",
            manager.stats.ema_jitter
        );

        // clean_streak should have built up because jitter is consistent
        // (below the adaptive threshold of ema_jitter * 1.5 + 1.0).
        assert!(
            manager.stats.clean_streak >= 50,
            "Expected clean_streak >= 50 on consistent 2.4GHz jitter, got {}",
            manager.stats.clean_streak
        );

        // stability_ratio should be meaningfully positive.
        assert!(
            manager.stability_ratio() > 0.1,
            "Expected stability_ratio > 0.1, got {}",
            manager.stability_ratio()
        );
    }

    #[test]
    fn should_not_oscillate_target_under_high_jitter_variance() {
        let (mut manager, mut encoder, mut prod, mut cons) = setup_env();
        let base_time = Instant::now();

        // Feed 500 packets with wildly varying inter-arrival times
        // to simulate a terrible 2.4GHz connection.
        for i in 1..=500u64 {
            let mut pkt = make_packet(&mut encoder, i, base_time);
            // Alternate: even packets arrive 0ms late, odd packets arrive 80ms late.
            let jitter_ms = if i % 2 == 0 { 0u64 } else { 80 };
            pkt.arrival_time = base_time
                + std::time::Duration::from_millis(i * MILLIS_PER_FRAME as u64 + jitter_ms);
            assert!(prod.try_push(pkt).is_ok());
        }
        manager.ingest_packets(&mut cons);

        // Now simulate 200 process_next_frame calls and count how many times
        // effective_target changes.
        manager.is_prebuffering = false;
        // Pre-fill buffer so we don't starve.
        for i in 501..=700u64 {
            let pkt = make_packet(&mut encoder, i, base_time);
            assert!(prod.try_push(pkt).is_ok());
        }
        manager.ingest_packets(&mut cons);

        let mut output = vec![0.0; OPUS_FRAME_SAMPLES];
        let mut changes = 0u32;
        let mut last_target = manager.effective_target;
        for _ in 0..200 {
            manager.fill_output(&mut output, 1.0);
            if manager.effective_target != last_target {
                changes += 1;
                last_target = manager.effective_target;
            }
        }

        // With hysteresis + quantization + rate-limiting, the target should change
        // fewer than 15 times across 200 callbacks (vs. potentially every callback before).
        assert!(
            changes < 15,
            "Expected fewer than 15 target changes across 200 callbacks, got {changes}"
        );
    }

    #[test]
    fn flush_with_crossfade_should_produce_output_without_decoder_reset() {
        let (mut manager, mut encoder, mut prod, mut cons) = setup_env();
        let base_time = Instant::now();

        // Fill buffer way beyond target.
        for i in 1..=100u64 {
            assert!(
                prod.try_push(make_packet(&mut encoder, i, base_time))
                    .is_ok()
            );
        }
        manager.ingest_packets(&mut cons);
        manager.is_prebuffering = false;

        // Decode one packet to populate decode_buf.
        let mut output = vec![0.0; OPUS_FRAME_SAMPLES];
        manager.fill_output(&mut output, 1.0);

        // Record opus_next_expected_seq before flush.
        let seq_before = manager.opus_next_expected_seq;

        // Manually flush down to 10 frames using the crossfade path.
        manager.flush_with_crossfade(10);

        // The decoder state should NOT have been hard-reset.
        // opus_next_expected_seq should still be Some (not None, which reset_state sets).
        assert!(
            manager.opus_next_expected_seq.is_some(),
            "opus_next_expected_seq should be Some after crossfade flush"
        );

        // The sequence should have advanced (decoder was fed through skipped frames).
        assert!(
            manager.opus_next_expected_seq > seq_before,
            "opus_next_expected_seq should have advanced past flushed frames"
        );

        // Buffer should be at or below the flush target.
        assert!(
            manager.buffer.occupied_count() <= 10,
            "Buffer should be <= 10 after flush, got {}",
            manager.buffer.occupied_count()
        );
    }
}
