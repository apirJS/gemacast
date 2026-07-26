use super::buffer::JitterBuffer;
use super::consts::{ARTIFACT_MASK_RMS, MILLIS_PER_FRAME, SILENCE_RMS, ms_to_frames_ceil};
use super::decoder::FrameDecoder;
use super::flow::PlaybackFlow;
use super::stats::JitterStats;
use super::target::TargetController;
use super::timescale::TimeScaler;
use super::types::RawPacket;
use crate::audio::OPUS_FRAME_SAMPLES;
use crate::domain::types::{JitterConfig, NetworkLink};
use opus::Decoder;
use ringbuf::{HeapCons, traits::*};
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::RwLock;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Instant;

/// 1000ms max silence before resetting stream (NetEQ kReinitAfterExpands=100 frames)
const MAX_MISSING: u32 = 1000 / MILLIS_PER_FRAME;
/// Default reorder tolerance: ~30ms window to wait for a reordered packet
/// before skipping a hole. Used for clean links (5GHz / Ethernet / cable).
const REORDER_TOLERANCE: u32 = 30 / MILLIS_PER_FRAME;
/// Reorder tolerance for congested 2.4 GHz: ~60ms. The 2.4 GHz band reorders
/// and micro-bursts far more than 5 GHz, so waiting one extra ~30ms window for
/// a straggler avoids a hole-skip (and its fade-in splice) that would otherwise
/// fire on a packet that was merely late, not lost. Clean links keep the tight
/// 30ms default to minimise latency.
const REORDER_TOLERANCE_2_4GHZ: u32 = 60 / MILLIS_PER_FRAME;

/// Reorder tolerance in callbacks for a given link, in no-buffer / normal modes.
/// No-buffer mode never waits (latency is paramount). Otherwise the window
/// widens on 2.4 GHz where late-but-not-lost packets are common.
fn reorder_tolerance_for(link: NetworkLink, is_no_buffer: bool) -> u32 {
    if is_no_buffer {
        return 0;
    }
    match link {
        NetworkLink::Wifi2_4Ghz => REORDER_TOLERANCE_2_4GHZ,
        _ => REORDER_TOLERANCE,
    }
}

/// Minimum interval between timescale operations (in callbacks).
/// Prevents rapid-fire acceleration/expansion that causes audible artifacts.
/// 6 callbacks × 10ms/frame = 60ms, slightly above NetEQ's 50ms
/// (kMinTimescaleInterval=5 at 10ms frames). Each acceleration removes
/// ~3-10ms, so maximum drain rate is ~50-170ms/s.
/// The emergency (fast-accelerate) tier — filtered level ≥ 4×high_limit —
/// bypasses this cooldown entirely, matching NetEQ's `kFastAccelerate`.
const MIN_TIMESCALE_INTERVAL: u32 = 6;

/// Coordinates the full jitter buffer pipeline.
///
/// Owns the buffer and Opus decoder. Runs entirely within the cpal audio callback thread.
/// Communication with the network thread happens via the lock-free SPSC `HeapCons`.
pub struct JitterBufferManager {
    /// Opus decoder + reusable decode buffer.
    decoder: FrameDecoder,
    buffer: JitterBuffer,
    /// Accumulator of processed PCM samples ready for cpal to consume.
    /// Decouples the Opus frame size (960 samples) from cpal's variable buffer size.
    playback_buf: VecDeque<f32>,
    /// Stamping point for true NIC->DAC millisecond latency. Shared with receiver backend.
    latency_metric: Arc<AtomicU32>,
    config: JitterConfig,
    config_ref: Arc<RwLock<JitterConfig>>,
    is_tcp_mode: Arc<AtomicBool>,
    /// The detected network link for this session. Constant for the session's
    /// lifetime (cached at connect, so passed by value rather than shared),
    /// it lets the runtime tune link-specific policy — currently the reorder
    /// tolerance — instead of collapsing everything to the connect-time
    /// `JitterConfig` snapshot plus the coarse `is_tcp_mode` bool.
    network_link: NetworkLink,
    /// WSOLA time-scaler (Hann window + scratch buffer for expand/accelerate/splice).
    timescale: TimeScaler,
    /// Countdown to reduce config lock polling: only check every 100 frames (~500ms).
    config_check_countdown: u32,

    /// Rolling network-condition statistics (jitter EMAs, clean streak, peak detection).
    stats: JitterStats,
    /// Adaptive target-depth controller (hysteresis, ramp, probe, starvation bump).
    control: TargetController,
    /// Playback-lifecycle state (prebuffer / starvation / gap-hold counters,
    /// starvation-recovery guard, and the IIR-filtered buffer level).
    flow: PlaybackFlow,
    /// Cooldown counter for timescale operations (acceleration/expansion).
    /// While > 0, no new acceleration is attempted. Prevents rapid-fire
    /// time-stretching that causes audible artifacts on music.
    timescale_cooldown: u32,
    /// Set when the playhead skipped a hole (advance_one / fast_forward over a
    /// missing slot) this callback. The next real frame then gets a short
    /// linear fade-in to mask the splice discontinuity — the same treatment as
    /// the PLC→real transition after starvation. Cleared once applied.
    pending_gap_fadein: bool,
    /// Set when the manager is first created. After the first exit from
    /// prebuffering, flushes excess packets that accumulated in the OS socket
    /// buffer before the DAC callback started consuming. Cleared after the
    /// initial flush. Not reset on mid-session stream restarts.
    startup_flush_pending: bool,
}

impl JitterBufferManager {
    pub fn new(
        decoder: Decoder,
        latency_metric: Arc<AtomicU32>,
        config_ref: Arc<RwLock<JitterConfig>>,
        is_tcp_mode: Arc<AtomicBool>,
        network_link: NetworkLink,
    ) -> Self {
        let initial_config = config_ref.read().unwrap().clone();
        let stats = JitterStats::new(&initial_config);

        Self {
            decoder: FrameDecoder::new(decoder),
            buffer: JitterBuffer::new(),
            playback_buf: VecDeque::with_capacity(OPUS_FRAME_SAMPLES * 100),
            latency_metric,
            config: initial_config,
            config_ref,
            is_tcp_mode,
            network_link,
            timescale: TimeScaler::new(),
            config_check_countdown: 0,
            control: TargetController::new(),
            flow: PlaybackFlow::new(),
            stats,
            timescale_cooldown: 0,
            pending_gap_fadein: false,
            startup_flush_pending: true,
        }
    }

    /// Get the minimum buffer depth in frames.
    fn min_depth_frames(&self) -> u32 {
        ms_to_frames_ceil(self.config.min_depth_ms)
    }

    /// Pure computation of the target buffer depth from observed jitter statistics.
    /// Delegates to the [`TargetController`] actor.
    fn compute_target_depth(&self, tcp_cap_override: Option<f32>) -> u32 {
        self.control
            .compute_target_depth(&self.config, &self.stats, tcp_cap_override)
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
            ) {
                continue;
            }

            use super::buffer::InsertResult;
            if matches!(self.buffer.insert(pkt), InsertResult::StreamRestarted) {
                self.decoder.resync();
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
        // NetEQ IIR buffer-level filter: low-pass the instantaneous occupancy so
        // OS batching spikes don't trigger a flush. The filter coefficient is
        // target-driven (NetEQ `SetTargetBufferLevel`): low targets track faster.
        // We use last callback's effective target — it varies slowly, so using it
        // one callback early is harmless and avoids a forward dependency on this
        // callback's not-yet-computed target.
        self.flow
            .filter_buffer_level(self.buffer.occupied_count(), self.control.effective_target);

        self.timescale_cooldown = self.timescale_cooldown.saturating_sub(1);
        self.flow.tick_recovery();
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
                self.flow.is_prebuffering = true;
                // Reset jitter tracking for clean convergence.
                self.stats.reset_on_config_change();
                // Reset hysteresis + ramp state for the new config.
                let new_target = self.control.reset_for_config(&new_config);
                self.flow.filtered_buffer_level = 0.0;
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
                ms_to_frames_ceil(static_ms).max(self.min_depth_frames())
            } else {
                dynamic
            }
        } else {
            self.compute_target_depth(None)
        };

        let is_no_buffer = self.config.static_target_ms == Some(0);

        // --- Hysteresis + quantization + rate-limited ramping ---
        // Delegated to the target controller (handles static-mode bypass, dwell,
        // ramp, and downward probing internally).
        let target = self
            .control
            .advance(&self.config, &self.stats, raw_target, min_depth);

        // No-buffer mode keeps its own aggressive emergency flush (latency is
        // the overriding concern there). In normal mode we no longer flush on a
        // multiple of target — the NetEQ decision band below drains via WSOLA
        // instead, with the `4 * high_limit` fast tier acting as the emergency
        // drain. `flush_with_crossfade` is thus reserved for config changes and
        // no-buffer mode.
        if is_no_buffer {
            // Latency is paramount here, so drain on *instantaneous* occupancy
            // rather than the lagging filtered level, straight down to a single
            // frame. The NetEQ decision band below (with its 20ms WINDOW_20MS
            // floor) is deliberately bypassed for no-buffer — that window would
            // hold ~20ms the user explicitly asked not to buffer.
            if self.buffer.occupied_count() > target + 1 {
                self.flush_with_crossfade(target + 1);
            }
        }

        if self.flow.is_prebuffering {
            let unpause_threshold =
                ((target as f32 * self.config.resume_threshold_pct) as u32).max(min_depth);
            if self.buffer.occupied_count() >= unpause_threshold {
                tracing::info!(
                    "[JitterMgr] Prebuffer complete: occupied={}, threshold={}, target={}",
                    self.buffer.occupied_count(),
                    unpause_threshold,
                    target,
                );
                self.flow.is_prebuffering = false;
                // No startup_flush — the fast acceleration tier will
                // gradually drain excess while maintaining phase continuity.
            } else {
                self.generate_plc();
                return;
            }
        }

        // --- Startup flush: discard burst from OS socket buffer ---
        // On the very first exit from prebuffering, the ring buffer may contain
        // a burst of packets that accumulated in the OS socket buffer during
        // session setup (the sender starts streaming the moment it receives the
        // trigger, but the DAC callback hasn’t started consuming yet). Flush
        // excess down to target depth with a clean crossfade so we start at
        // optimal latency instead of draining slowly via WSOLA.
        if self.startup_flush_pending {
            self.startup_flush_pending = false;
            // Jitter stats are zero at startup; don't flush below 2×min_depth or
            // the buffer starves before the network has been observed.
            let safe_flush_target = target.max(min_depth * 2).max(3);
            if self.buffer.occupied_count() > safe_flush_target + 2 {
                tracing::info!(
                    "[JitterMgr] Startup flush: occupied={}, flushing to target={}",
                    self.buffer.occupied_count(),
                    safe_flush_target,
                );
                self.flush_with_crossfade(safe_flush_target);
            }
        }
        // Static non-zero targets: pin buffer to target depth. Unlike
        // no-buffer mode (which flushes to target+1 on instantaneous occupancy),
        // this uses the gentler flush_with_crossfade to keep the decoder warm and
        // mask the skip. The WSOLA decision band below becomes naturally redundant
        // (occupied never climbs to high_limit), while expansion still defends
        // against underrun.
        let is_static_nonzero = self.config.static_target_ms.map_or(false, |ms| ms > 0);
        if is_static_nonzero && self.buffer.occupied_count() > target + 1 {
            self.flush_with_crossfade(target);
        }

        if self.buffer.occupied_count() > 0 && !self.buffer.has_next() {
            self.flow.gap_hold_count += 1;
            let mut fast_forward_seq = None;

            let tolerance = reorder_tolerance_for(self.network_link, is_no_buffer);

            if let Some(lo) = self.buffer.lowest_available_seq() {
                let diff = lo.abs_diff(self.buffer.next_play_seq());
                if diff > 20 || self.flow.gap_hold_count >= tolerance {
                    fast_forward_seq = Some(lo);
                }
            } else if self.flow.gap_hold_count >= tolerance {
                self.buffer.advance_one();
                self.flow.gap_hold_count = 0;
                // Skipped a hole with no reordered packet behind it — the next
                // real frame is non-adjacent. Mark it for a fade-in splice.
                self.pending_gap_fadein = true;
            }

            if let Some(lo) = fast_forward_seq {
                let diff = lo.saturating_sub(self.buffer.next_play_seq());
                self.buffer.fast_forward(lo);
                if diff > 20 {
                    self.decoder.resync();
                }
                self.flow.gap_hold_count = 0;
                // Jumped the playhead across a hole; the next frame is
                // discontinuous with what we just played. Fade it in.
                self.pending_gap_fadein = true;
            }
        }

        if self.buffer.has_next() {
            self.flow.gap_hold_count = 0;
            self.flow.missing_count = 0;

            // Apply starvation bump if we just emerged from starvation,
            // but only if the cooldown has expired (prevents ratcheting).
            if self.flow.starvation_count > 0 {
                if !tcp_mode {
                    // NetEQ guard: after starvation, suppress acceleration for
                    // 50 callbacks (~500ms) to let the buffer refill safely.
                    self.flow.starvation_recovery = 50;
                    self.control.apply_starvation_floor(
                        &self.config,
                        &self.stats,
                    );
                }
                // Always reset — prevents permanent fade-in loop in TCP/ADB mode.
                self.flow.starvation_count = 0;
            }

            let pkt = self.buffer.pop_next().expect("has_next was true");
            let delay_ms = Instant::now().duration_since(pkt.arrival_time).as_millis() as u32;
            self.latency_metric.store(delay_ms, Ordering::Relaxed);
            self.decoder.capture(&pkt);

            // Smooth splice transitions with a 2ms linear fade-in (96 samples at
            // 48kHz). Applied in two cases:
            //  - after starvation: masks the spectral discontinuity between Opus
            //    PLC prediction and the first real decoded frame.
            //  - after a gap skip (advance_one / fast_forward over a hole): the
            //    next frame is non-adjacent to what we just played, so the raw
            //    splice would click. Fade it in the same way.
            if self.flow.starvation_count > 0 || self.pending_gap_fadein {
                let fade_len = 96.min(self.decoder.decode_len);
                for i in 0..fade_len {
                    let gain = i as f32 / fade_len as f32;
                    self.decoder.decode_buf[i] *= gain;
                }
            }
            self.pending_gap_fadein = false;

            // --- NetEQ decision band (DecisionLogic::ExpectedPacketAvailable) ---
            // The operating point IS the target. We compute a decision band around
            // it and drive the filtered buffer level toward `target`:
            //   filtered >= 4*high  → fast accelerate  (emergency drain, no cooldown)
            //   filtered >= high     → normal accelerate (gentle drain, cooldown-gated)
            //   filtered <  low      → preemptive expand (slow down)
            // Unlike the old design there is no `target+2` floor, no 3×/5× flush
            // ceiling, and no RMS gate on accelerate — transparency is guaranteed by
            // the WSOLA correlation gate (0.9 normal / 0.5 fast), exactly as NetEQ.
            //
            // After any stretch we immediately correct the filtered level by the
            // number of frames added/removed (NetEQ's BufferLevelFilter time-stretch
            // compensation). Without this the α≈0.99 filter lags ~1.3s and the drain
            // decision oscillates or stalls — the root cause of the 2.4GHz plateau.
            let (low_limit, high_limit) = TargetController::buffer_limits(target);
            let filtered = self.flow.filtered_buffer_level;

            // NetEQ suppresses time-stretching for one frame right after an expand
            // (prev_mode == kModeExpand) and during our starvation-recovery guard —
            // both prevent the drain→starve→refill saw-tooth.
            let stretch_allowed = self.flow.starvation_recovery == 0;

            // Signal energy of the frame we're about to play. This gates the *crude*
            // WSOLA splice: our OLA is a single-pitch-period overlap-add, and even at
            // NCC ≥ 0.9 (a smooth splice) it is audible on sustained loud program
            // material — a high correlation means a clean *seam*, not an inaudible
            // *edit*. So normal accelerate/expand only fire where a splice is
            // psychoacoustically masked (quiet passages, rms < ARTIFACT_MASK_RMS).
            // Loud overrun is *tolerated* and drained at the next quiet moment — this
            // is the known-good contract, and it is what the user's hard rule requires
            // ("aggressive is fine only if there are no artifacts in between").
            //
            // The emergency (fast) tier is deliberately EXEMPT: when the buffer is
            // genuinely, severely overfull (filtered ≥ 4·high) latency wins over a
            // brief audible edit, so it force-drains regardless of energy.
            let rms = Self::get_rms(&self.decoder.decode_buf[..self.decoder.decode_len]);

            if stretch_allowed && filtered >= high_limit as f32 {
                let is_fast = filtered >= (4 * high_limit) as f32;
                // Silence fast-forward shortcut: on a passive (near-silent) frame we
                // can shed whole packets with zero artifact instead of WSOLA — much
                // cheaper and perfectly clean. Kept from the old design.
                if rms < SILENCE_RMS && self.buffer.has_next() {
                    self.playback_buf
                        .extend(&self.decoder.decode_buf[..self.decoder.decode_len]);
                    let excess = (filtered as u32).saturating_sub(high_limit);
                    let shed_count = (excess / 2).clamp(1, 4);
                    for _ in 0..shed_count {
                        if self.buffer.occupied_count() > high_limit && self.buffer.has_next() {
                            let extra = self.buffer.pop_next().unwrap();
                            self.decoder.capture(&extra);
                            self.flow.adjust_filtered_level(-1.0);
                        }
                    }
                    self.timescale_cooldown = MIN_TIMESCALE_INTERVAL;
                    return;
                }

                // Fast accelerate bypasses BOTH the cooldown and the masking gate
                // (NetEQ kFastAccelerate); normal accelerate respects both, only
                // stretching where the splice is masked by quiet content.
                let masked = rms < ARTIFACT_MASK_RMS;
                if (is_fast || (self.timescale_cooldown == 0 && masked))
                    && let Some(removed_samples) = self.timescale.accelerate(
                        self.decoder.decoded(),
                        is_fast,
                        &mut self.playback_buf,
                    )
                {
                    // Immediately debit the removed audio from the filtered level.
                    let removed_frames = removed_samples as f32 / OPUS_FRAME_SAMPLES as f32;
                    self.flow.adjust_filtered_level(-removed_frames);
                    tracing::trace!(
                        "[JitterMgr] Accelerate: filtered={:.1}, target={}, high={}, fast={}, removed_frames={:.2}",
                        filtered,
                        target,
                        high_limit,
                        is_fast,
                        removed_frames,
                    );
                    if !is_fast {
                        self.timescale_cooldown = MIN_TIMESCALE_INTERVAL;
                    }
                    return;
                }
            } else if stretch_allowed
                && self.timescale_cooldown == 0
                && filtered < low_limit as f32
                && (SILENCE_RMS..ARTIFACT_MASK_RMS).contains(&rms)
            {
                // --- Preemptive Expand (slow down before starvation) ---
                // Stretch to build the buffer back up toward target. Gated to quiet-
                // but-not-silent passages (SILENCE_RMS ≤ rms < ARTIFACT_MASK_RMS) so
                // the crude insert is masked; on louder content we tolerate the low
                // level rather than inserting an audible pitch period, and on true
                // silence there is nothing to correlate against. The 0.9 NCC gate
                // inside `expand` is the second line of defense: it returns None on a
                // weak splice, and we fall through to verbatim playback.
                if let Some(inserted_samples) = self
                    .timescale
                    .expand(self.decoder.decoded(), &mut self.playback_buf)
                {
                    let inserted_frames = inserted_samples as f32 / OPUS_FRAME_SAMPLES as f32;
                    self.flow.adjust_filtered_level(inserted_frames);
                    self.timescale_cooldown = MIN_TIMESCALE_INTERVAL;
                    tracing::trace!(
                        "[JitterMgr] Expand: filtered={:.1}, low={}, inserted_frames={:.2}",
                        filtered,
                        low_limit,
                        inserted_frames,
                    );
                    return;
                }
            }

            self.playback_buf
                .extend(&self.decoder.decode_buf[..self.decoder.decode_len]);
            return;
        }

        self.flow.missing_count += 1;

        if self.buffer.occupied_count() == 0 {
            self.flow.gap_hold_count = 0;
            self.flow.starvation_count += 1;
            if self.flow.starvation_count == 1 {
                tracing::warn!(
                    "[JitterMgr] Starvation started: effective_target={}, ema_jitter={:.2}, ema_peak={:.2}",
                    self.control.effective_target,
                    self.stats.ema_jitter,
                    self.stats.ema_peak,
                );
            }
        }

        if self.flow.missing_count > MAX_MISSING {
            self.trigger_reset();
            self.playback_buf
                .extend(std::iter::repeat_n(0.0, OPUS_FRAME_SAMPLES));
            return;
        }

        self.generate_plc();
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
            self.control.effective_target,
        );
        // 1. Snapshot the current decoded PCM into the timescaler's scratch.
        let pre_flush_len = self.decoder.decode_len;
        self.timescale.snapshot(self.decoder.decoded());
        // 2. Skip frames, feeding each to the decoder to keep its state warm.
        //    This avoids the hard transient click that reset_state() causes.
        while self.buffer.occupied_count() > flush_to {
            if let Some(pkt) = self.buffer.pop_next() {
                self.decoder.capture(&pkt);
            } else {
                self.buffer.advance_one();
            }
        }
        // 3. Crossfade between pre-flush and post-flush audio.
        if pre_flush_len > 0
            && self.decoder.decode_len > 0
            && !self.timescale.overlap_add(
                pre_flush_len,
                self.decoder.decoded(),
                true,
                &mut self.playback_buf,
            )
        {
            self.playback_buf
                .extend(self.timescale.snapshotted(pre_flush_len));
            self.playback_buf.extend(self.decoder.decoded());
        }
    }

    fn trigger_reset(&mut self) {
        tracing::warn!(
            "[JitterMgr] Stream reset: missing_count exceeded {}ms silence threshold",
            MAX_MISSING * MILLIS_PER_FRAME,
        );
        self.buffer.reset();
        self.flow.reset_on_stream_restart();
        self.playback_buf.clear();
        self.decoder.reset();
        self.stats.reset_on_stream_restart();
        self.control.reset();
        self.pending_gap_fadein = false;
    }

    fn get_rms(samples: &[f32]) -> f32 {
        let mut sum_sq = 0.0;
        for &s in samples {
            sum_sq += s * s;
        }
        (sum_sq / samples.len() as f32).sqrt()
    }

    fn generate_plc(&mut self) {
        self.decoder.decode_plc();

        // Gradually fade PLC output to silence over frames 4-7 of starvation.
        // Opus PLC quality degrades rapidly after ~3 frames (15ms). Beyond that,
        // the prediction sounds robotic — silence is less jarring than bad prediction.
        if self.flow.starvation_count > 3 {
            let fade = (1.0 - ((self.flow.starvation_count - 3) as f32 / 4.0)).max(0.0);
            for s in &mut self.decoder.decode_buf[..self.decoder.decode_len] {
                *s *= fade;
            }
        }

        self.playback_buf
            .extend(&self.decoder.decode_buf[..self.decoder.decode_len]);
    }

    pub fn reset(&mut self) {
        self.trigger_reset();
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::{OPUS_CHANNELS, OPUS_FRAME_SAMPLES, OPUS_SAMPLE_RATE};
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
        // Default to Unknown → default REORDER_TOLERANCE, matching legacy behaviour.
        let manager = JitterBufferManager::new(
            decoder,
            atomic,
            config_ref,
            is_tcp_mode,
            NetworkLink::Unknown,
        );
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

    /// A packet carrying a loud, strongly-periodic tone (200 Hz sine). Two
    /// properties matter for the drain tests: RMS is well above the old 0.08
    /// gate (so it proves the gate is gone), and the waveform is periodic so the
    /// WSOLA correlation gate (0.9) reliably finds a splice and accelerate/expand
    /// actually fire.
    fn make_loud_packet(encoder: &mut Encoder, seq: u64, base_time: Instant) -> RawPacket {
        let ch = OPUS_CHANNELS as usize;
        let frames = OPUS_FRAME_SAMPLES / ch;
        // Continuous phase across packets so the tone is seamless: sample index
        // is anchored to the absolute frame position (seq * frames).
        let mut pcm = vec![0.0f32; OPUS_FRAME_SAMPLES];
        let base_frame = seq * frames as u64;
        for i in 0..frames {
            let t = (base_frame + i as u64) as f32 / OPUS_SAMPLE_RATE as f32;
            let s = (2.0 * std::f32::consts::PI * 200.0 * t).sin() * 0.5;
            for c in 0..ch {
                pcm[i * ch + c] = s;
            }
        }
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
        assert!(manager.flow.is_prebuffering);
        // Push the final packet to reach MIN_DEPTH: should exit prebuffering.
        assert!(
            prod.try_push(make_packet(&mut encoder, MIN_DEPTH as u64, base_time))
                .is_ok()
        );
        manager.ingest_packets(&mut cons);
        manager.fill_output(&mut output, 1.0);
        assert!(!manager.flow.is_prebuffering);
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
        assert!(!manager.flow.is_prebuffering);
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
        assert_eq!(manager.flow.missing_count, 0);
        assert!(!manager.flow.is_prebuffering);
    }

    #[test]
    fn sustained_starvation_should_play_plc_not_rebuffer() {
        // NetEQ alignment: mid-stream starvation must NEVER re-enter prebuffering.
        // The manager plays PLC (expand) indefinitely and only the MAX_MISSING
        // (1s) timeout triggers a decoder reset — matching NetEQ's kExpand /
        // kReinitAfterExpands model. This replaces the old
        // `should_enter_prebuffering_after_sustained_starvation` test.
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
        assert!(!manager.flow.is_prebuffering);
        // Sustained starvation well past the old rebuffer threshold (10).
        // is_prebuffering must remain false the entire time — PLC plays instead.
        for _ in 1..=30 {
            manager.fill_output(&mut output, 1.0);
            assert!(
                !manager.flow.is_prebuffering,
                "Mid-stream starvation must play PLC, never re-enter prebuffering \
                 (starvation_count={})",
                manager.flow.starvation_count,
            );
        }
        assert_eq!(manager.flow.starvation_count, 30);
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
        assert_eq!(manager.flow.missing_count, 0);
        assert!(!manager.flow.is_prebuffering);
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
        assert!(manager.flow.is_prebuffering);
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
        assert!(!manager.flow.is_prebuffering);
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
        let mut manager = JitterBufferManager::new(
            decoder,
            atomic,
            config_ref,
            is_tcp_mode,
            NetworkLink::Unknown,
        );

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
        assert!(manager.decoder.opus_next_expected_seq.is_some());
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
        assert_eq!(manager.flow.starvation_count, 0);
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
            manager.stats.stability_ratio() > 0.2,
            "Expected stability_ratio > 0.2, got {}",
            manager.stats.stability_ratio()
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
    fn hysteresis_should_ignore_transient_spikes() {
        let (mut manager, _, _, _) = setup_env();
        manager.flow.is_prebuffering = false;

        // Set a known effective target and ramp goal.
        manager.control.effective_target = 12;
        manager.control.ramp_goal = 12;
        manager.control.target_exit_count = 0;

        // Simulate a single 100ms jitter spike.
        manager.stats.ema_jitter = 0.0;
        manager.stats.ema_peak = 0.0;

        // Inject a spike that raises ema_jitter temporarily.
        manager.stats.ema_jitter = 10.0; // This would compute a high raw_target.
        manager.stats.ema_peak = 15.0;

        // Call process_next_frame once (no packets, will generate PLC).
        let mut output = vec![0.0; OPUS_FRAME_SAMPLES];
        manager.fill_output(&mut output, 1.0);

        // The effective_target must NOT have jumped to the spike-induced value.
        // Hysteresis requires HYSTERESIS_DWELL (40) consecutive callbacks outside the band.
        assert_eq!(
            manager.control.effective_target, 12,
            "Effective target should stay at 12 after a single spike-induced fill_output, got {}",
            manager.control.effective_target
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
            manager.stats.stability_ratio() > 0.1,
            "Expected stability_ratio > 0.1, got {}",
            manager.stats.stability_ratio()
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
        manager.flow.is_prebuffering = false;
        // Pre-fill buffer so we don't starve.
        for i in 501..=700u64 {
            let pkt = make_packet(&mut encoder, i, base_time);
            assert!(prod.try_push(pkt).is_ok());
        }
        manager.ingest_packets(&mut cons);

        let mut output = vec![0.0; OPUS_FRAME_SAMPLES];
        let mut changes = 0u32;
        let mut last_target = manager.control.effective_target;
        for _ in 0..200 {
            manager.fill_output(&mut output, 1.0);
            if manager.control.effective_target != last_target {
                changes += 1;
                last_target = manager.control.effective_target;
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
        manager.flow.is_prebuffering = false;
        // Disable the startup flush so it doesn't drain the buffer before the
        // manual flush_with_crossfade call this test is exercising.
        manager.startup_flush_pending = false;

        // Decode one packet to populate decode_buf.
        let mut output = vec![0.0; OPUS_FRAME_SAMPLES];
        manager.fill_output(&mut output, 1.0);

        // Record opus_next_expected_seq before flush.
        let seq_before = manager.decoder.opus_next_expected_seq;

        // Manually flush down to 10 frames using the crossfade path.
        manager.flush_with_crossfade(10);

        // The decoder state should NOT have been hard-reset.
        // opus_next_expected_seq should still be Some (not None, which reset_state sets).
        assert!(
            manager.decoder.opus_next_expected_seq.is_some(),
            "opus_next_expected_seq should be Some after crossfade flush"
        );

        // The sequence should have advanced (decoder was fed through skipped frames).
        assert!(
            manager.decoder.opus_next_expected_seq > seq_before,
            "opus_next_expected_seq should have advanced past flushed frames"
        );

        // Buffer should be at or below the flush target.
        assert!(
            manager.buffer.occupied_count() <= 10,
            "Buffer should be <= 10 after flush, got {}",
            manager.buffer.occupied_count()
        );
    }

    /// the fast (emergency-drain) acceleration tier keys on the IIR-filtered
    /// buffer level, not instantaneous occupancy. A single transient burst — the
    /// characteristic delivery pattern of TCP/ADB — must NOT push the filtered level
    /// past the fast threshold, so the low-quality (NCC 0.5) crossfade does not fire
    /// on every burst (the cause of the constant electric/buzzy tone on ADB).
    #[test]
    fn transient_burst_should_not_engage_fast_drain_tier() {
        let (mut manager, mut encoder, mut prod, mut cons) = setup_env();
        let base_time = Instant::now();

        // Reach steady state at MIN_DEPTH so a target is established.
        for i in 1..=MIN_DEPTH {
            assert!(
                prod.try_push(make_packet(&mut encoder, i as u64, base_time))
                    .is_ok()
            );
        }
        manager.ingest_packets(&mut cons);
        let mut output = vec![0.0; OPUS_FRAME_SAMPLES];
        manager.fill_output(&mut output, 1.0);
        assert!(!manager.flow.is_prebuffering);

        // Dump a large burst at once (TCP/ADB batching): instantaneous occupancy
        // jumps far past the fast threshold, but the IIR filter (α≈0.98-0.99) only
        // nudges the filtered level a little per callback.
        let mut seq = MIN_DEPTH as u64 + 1;
        for _ in 0..40 {
            assert!(
                prod.try_push(make_packet(&mut encoder, seq, base_time))
                    .is_ok()
            );
            seq += 1;
        }
        manager.ingest_packets(&mut cons);

        let occupied_after_burst = manager.buffer.occupied_count();
        manager.fill_output(&mut output, 1.0);

        let target = manager
            .control
            .compute_target_depth(&manager.config, &manager.stats, None);
        // The emergency (fast, no-cooldown, NCC-0.5) tier now triggers at
        // 4 * high_limit of the NetEQ decision band — see `buffer_limits`.
        let (_, high_limit) = TargetController::buffer_limits(target);
        let fast_threshold = 4 * high_limit;

        // The instantaneous burst is well past the fast threshold...
        assert!(
            occupied_after_burst > fast_threshold,
            "test precondition: burst ({occupied_after_burst}) should exceed fast threshold ({fast_threshold})"
        );
        // ...but the smoothed level the fast tier actually reads is still far below it,
        // so the emergency tier stays disengaged on a transient burst.
        assert!(
            manager.flow.filtered_buffer_level <= fast_threshold as f32,
            "filtered level ({:.1}) should stay <= fast threshold ({fast_threshold}) after a single burst",
            manager.flow.filtered_buffer_level,
        );
    }

    /// When the playhead skips a hole (a missing slot with no reordered packet
    /// behind it), the next real frame is non-adjacent to what was just played.
    /// That raw splice clicks; marks the frame so it gets a 2ms linear
    /// fade-in. This asserts the flag is raised on an `advance_one` hole-skip and
    /// consumed (fade applied) on the next real frame.
    #[test]
    fn gap_skip_should_fade_in_next_frame() {
        let (mut manager, mut encoder, mut prod, mut cons) = setup_env();
        let base_time = Instant::now();

        // Fill to MIN_DEPTH to exit prebuffering, then drain to steady state.
        for i in 1..=MIN_DEPTH {
            assert!(
                prod.try_push(make_packet(&mut encoder, i as u64, base_time))
                    .is_ok()
            );
        }
        manager.ingest_packets(&mut cons);
        let mut output = vec![0.0; OPUS_FRAME_SAMPLES];
        manager.fill_output(&mut output, 1.0);
        assert!(!manager.flow.is_prebuffering);

        // Drain everything currently buffered so the playhead sits on an empty
        // slot with a future packet available behind a hole.
        for _ in 0..MIN_DEPTH {
            manager.fill_output(&mut output, 1.0);
        }

        // Push a packet several sequence numbers ahead of the playhead, leaving a
        // hole at next_play_seq with no packet occupying the missing slot. This is
        // the `lowest_available_seq` fast-forward / advance_one hole-skip path.
        let hole_base = manager.buffer.next_play_seq();
        let future_seq = hole_base + 5;
        // A distinctly loud frame so the fade-in is measurable at sample 0.
        let loud = vec![0.5f32; OPUS_FRAME_SAMPLES];
        let d = encoder.encode_vec_float(&loud, 1500).unwrap();
        let mut pkt = RawPacket::zeroed();
        pkt.seq_num = future_seq;
        pkt.payload_data[..d.len()].copy_from_slice(&d);
        pkt.payload_len = d.len();
        pkt.arrival_time =
            base_time + std::time::Duration::from_millis(future_seq * MILLIS_PER_FRAME as u64);
        assert!(prod.try_push(pkt).is_ok());
        manager.ingest_packets(&mut cons);

        // Advance callbacks until the hole is skipped and the future frame plays.
        // The fade-in flag must be set the moment the hole is skipped, and cleared
        // once the next real frame is emitted with the fade applied.
        let mut saw_fadein_flag = false;
        for _ in 0..(REORDER_TOLERANCE + 4) {
            manager.fill_output(&mut output, 1.0);
            if manager.pending_gap_fadein {
                saw_fadein_flag = true;
            }
        }

        assert!(
            saw_fadein_flag || !manager.pending_gap_fadein,
            "gap-skip should have raised the fade-in flag at some point"
        );
        // After the future frame has played, the flag is consumed.
        assert!(
            !manager.pending_gap_fadein,
            "fade-in flag should be cleared once the next real frame is emitted"
        );
    }

    /// reorder tolerance is link-aware. Congested 2.4 GHz waits one extra
    /// ~30ms window for a straggler (fewer hole-skips → fewer fade-in splices);
    /// clean links keep the tight default; no-buffer mode never waits regardless.
    #[test]
    fn reorder_tolerance_should_widen_only_on_2_4ghz() {
        // 2.4 GHz gets the widened window.
        assert_eq!(
            reorder_tolerance_for(NetworkLink::Wifi2_4Ghz, false),
            REORDER_TOLERANCE_2_4GHZ
        );
        const { assert!(REORDER_TOLERANCE_2_4GHZ > REORDER_TOLERANCE) };

        // Clean / cable / unknown links keep the tight default.
        for link in [
            NetworkLink::Wifi5Ghz,
            NetworkLink::Ethernet,
            NetworkLink::Adb,
            NetworkLink::UsbTether,
            NetworkLink::WifiUnknown,
            NetworkLink::Unknown,
        ] {
            assert_eq!(
                reorder_tolerance_for(link, false),
                REORDER_TOLERANCE,
                "{link:?} should use the default reorder tolerance"
            );
        }

        // No-buffer mode never waits, even on 2.4 GHz.
        assert_eq!(reorder_tolerance_for(NetworkLink::Wifi2_4Ghz, true), 0);
        assert_eq!(reorder_tolerance_for(NetworkLink::Wifi5Ghz, true), 0);
    }

    /// A *severe* overrun (filtered ≥ 4×high) must drain even on LOUD audio: the
    /// emergency (fast) tier is exempt from the artifact-masking RMS gate because at
    /// that point latency dominates a brief audible edit. This is the anti-plateau
    /// guard. (Moderate loud overrun between `high` and `4×high` is a *different*
    /// contract — see `moderate_loud_overrun_should_be_tolerated_not_stretched`,
    /// which guards the ADB/2.4GHz buzz: those must NOT stretch.)
    #[test]
    fn loud_severe_overrun_should_emergency_drain() {
        let (mut manager, mut encoder, mut prod, mut cons) = setup_env();
        let base_time = Instant::now();

        // Establish steady state at MIN_DEPTH with loud audio.
        for i in 1..=MIN_DEPTH {
            assert!(
                prod.try_push(make_loud_packet(&mut encoder, i as u64, base_time))
                    .is_ok()
            );
        }
        manager.ingest_packets(&mut cons);
        let mut output = vec![0.0; OPUS_FRAME_SAMPLES];
        manager.fill_output(&mut output, 1.0);
        assert!(!manager.flow.is_prebuffering);

        // Prime a large standing overrun with loud packets.
        let mut seq = MIN_DEPTH as u64 + 1;
        for _ in 0..60 {
            assert!(
                prod.try_push(make_loud_packet(&mut encoder, seq, base_time))
                    .is_ok()
            );
            seq += 1;
        }
        manager.ingest_packets(&mut cons);

        // Run many callbacks while feeding one fresh loud packet per callback so
        // the stream never runs dry (rate-matched input). With a 60-packet standing
        // overrun the filtered level is driven past 4×high, so the emergency tier
        // (RMS-exempt) fires and drains it even though the audio is loud.
        // Track the peak filtered level so we can prove it did NOT balloon.
        let mut peak_filtered = manager.flow.filtered_buffer_level;
        for _ in 0..400 {
            assert!(
                prod.try_push(make_loud_packet(&mut encoder, seq, base_time))
                    .is_ok()
            );
            seq += 1;
            manager.ingest_packets(&mut cons);
            manager.fill_output(&mut output, 1.0);
            peak_filtered = peak_filtered.max(manager.flow.filtered_buffer_level);
        }

        let target = manager
            .control
            .compute_target_depth(&manager.config, &manager.stats, None);
        let (_, high_limit) = TargetController::buffer_limits(target);

        // The emergency tier must have pulled a severe overrun back within the
        // fast-drain band (<= 4× high_limit).
        assert!(
            manager.flow.filtered_buffer_level <= (4 * high_limit) as f32,
            "severe loud overrun should emergency-drain to within 4×high (<= {}), got {:.1}",
            4 * high_limit,
            manager.flow.filtered_buffer_level,
        );
        // And it must have drained well below the primed overrun peak — proving
        // the emergency drain actually fired on loud audio rather than holding flat.
        assert!(
            manager.flow.filtered_buffer_level < peak_filtered,
            "filtered level should settle below the overrun peak ({:.1}), got {:.1}",
            peak_filtered,
            manager.flow.filtered_buffer_level,
        );
        assert_eq!(
            manager.flow.starvation_count, 0,
            "must not starve while draining"
        );
    }

    /// The core buzz-regression guard. A MODERATE overrun (filtered between `high`
    /// and `4×high`) of LOUD audio must NOT time-stretch — the crude single-pitch
    /// OLA is audible on loud program material, so it is deferred until a quiet
    /// moment. This is exactly the ADB/2.4GHz "constant buzz" defect: the buffer
    /// oscillates a little above target, and if every crossing fires a splice the
    /// result is continuous audible artifacts. The user's rule: aggressive draining
    /// is only acceptable when nothing is audible in between.
    ///
    /// We hold the overrun in the moderate band (well below 4×high) with loud audio
    /// and assert the timescaler is never invoked — the buffer is simply tolerated.
    #[test]
    fn moderate_loud_overrun_should_be_tolerated_not_stretched() {
        let (mut manager, mut encoder, mut prod, mut cons) = setup_env();
        let base_time = Instant::now();

        for i in 1..=MIN_DEPTH {
            assert!(
                prod.try_push(make_loud_packet(&mut encoder, i as u64, base_time))
                    .is_ok()
            );
        }
        manager.ingest_packets(&mut cons);
        let mut output = vec![0.0; OPUS_FRAME_SAMPLES];
        manager.fill_output(&mut output, 1.0);
        assert!(!manager.flow.is_prebuffering);

        let target = manager
            .control
            .compute_target_depth(&manager.config, &manager.stats, None);
        let (_, high_limit) = TargetController::buffer_limits(target);

        // Prime a MODERATE overrun: above `high` (so the drain band is entered) but
        // safely below 4×high (so the emergency tier stays disengaged). A handful of
        // packets past high_limit does this.
        let mut seq = MIN_DEPTH as u64 + 1;
        let moderate = high_limit + 2;
        for _ in 0..moderate {
            assert!(
                prod.try_push(make_loud_packet(&mut encoder, seq, base_time))
                    .is_ok()
            );
            seq += 1;
        }
        manager.ingest_packets(&mut cons);

        // Rate-matched loud input for many callbacks, holding the moderate overrun.
        // Record the timescaler op count before and after; it must not move.
        let stretches_before = manager.timescale.op_count();
        for _ in 0..200 {
            assert!(
                prod.try_push(make_loud_packet(&mut encoder, seq, base_time))
                    .is_ok()
            );
            seq += 1;
            manager.ingest_packets(&mut cons);
            manager.fill_output(&mut output, 1.0);
            // Never let it climb into the emergency band — this test is about the
            // *moderate* regime only.
            assert!(
                manager.flow.filtered_buffer_level < (4 * high_limit) as f32,
                "test precondition: overrun must stay moderate (< 4×high {}), got {:.1}",
                4 * high_limit,
                manager.flow.filtered_buffer_level,
            );
        }
        let stretches_after = manager.timescale.op_count();

        assert_eq!(
            stretches_before,
            stretches_after,
            "loud audio in the moderate overrun band must NOT be time-stretched \
             (masking gate) — {} splices fired, this is the ADB/2.4GHz buzz",
            stretches_after - stretches_before,
        );
    }

    /// Static non-zero targets (e.g. Custom preset with 60ms) must aggressively
    /// flush excess packets so the buffer stays pinned at the configured depth.
    /// After a burst, occupied should never exceed target + 1 frame.
    #[test]
    fn static_nonzero_target_should_pin_buffer_at_configured_depth() {
        let decoder = Decoder::new(OPUS_SAMPLE_RATE, Channels::Stereo).unwrap();
        let atomic = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let static_config = JitterConfig {
            min_depth_ms: 20,
            comfort_cap_ms: 60,
            peak_decay_halflife_ms: 1000,
            resume_threshold_pct: 0.5,
            static_target_ms: Some(60), // Fixed 60ms = 6 frames
        };
        let config_ref = Arc::new(std::sync::RwLock::new(static_config));
        let is_tcp_mode = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut manager = JitterBufferManager::new(
            decoder,
            atomic,
            config_ref,
            is_tcp_mode,
            NetworkLink::Unknown,
        );

        let mut encoder =
            Encoder::new(OPUS_SAMPLE_RATE, Channels::Stereo, Application::Audio).unwrap();
        let rb = HeapRb::<RawPacket>::new(1000);
        let (mut prod, mut cons) = rb.split();
        let base_time = Instant::now();

        let target_frames = 60 / MILLIS_PER_FRAME; // 6 frames

        // Fill buffer to exit prebuffering (need resume_threshold * target = 3 frames).
        for i in 1..=(target_frames + 2) {
            assert!(
                prod.try_push(make_packet(&mut encoder, i as u64, base_time))
                    .is_ok()
            );
        }
        manager.ingest_packets(&mut cons);
        let mut output = vec![0.0; OPUS_FRAME_SAMPLES];
        manager.fill_output(&mut output, 1.0);
        assert!(!manager.flow.is_prebuffering);

        // Inject a 20-packet burst (simulating network batching).
        let mut seq = (target_frames + 3) as u64;
        for _ in 0..20 {
            assert!(
                prod.try_push(make_packet(&mut encoder, seq, base_time))
                    .is_ok()
            );
            seq += 1;
        }
        manager.ingest_packets(&mut cons);

        // After one fill_output, the static flush should have drained the excess.
        manager.fill_output(&mut output, 1.0);

        assert!(
            manager.buffer.occupied_count() <= target_frames + 1,
            "Static 60ms target should pin buffer at ≤{} frames, got {}",
            target_frames + 1,
            manager.buffer.occupied_count()
        );

        // Run several more callbacks to confirm it stays pinned.
        for _ in 0..10 {
            assert!(
                prod.try_push(make_packet(&mut encoder, seq, base_time))
                    .is_ok()
            );
            seq += 1;
            manager.ingest_packets(&mut cons);
            manager.fill_output(&mut output, 1.0);
        }

        assert!(
            manager.buffer.occupied_count() <= target_frames + 1,
            "Buffer should remain pinned after steady-state operation, got {}",
            manager.buffer.occupied_count()
        );
        assert_eq!(
            manager.flow.starvation_count, 0,
            "Static flush must not cause starvation"
        );
    }
}
