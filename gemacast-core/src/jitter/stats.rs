//! Network-observation actor: consumes packet arrival times and maintains the
//! jitter statistics (dual-EMA jitter, variance, clean streak, NetEQ peak
//! detection) that the [`super::target::TargetController`] reads to size the
//! buffer. Owns no buffer or decoder — it only observes.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use super::consts::{MILLIS_PER_FRAME, lerp};
use crate::domain::types::JitterConfig;

/// Rolling network-condition statistics derived from packet inter-arrival times.
pub(super) struct JitterStats {
    /// EWMA of inter-arrival jitter (frames).
    pub ema_jitter: f32,
    /// Slow-decay peak tracker for worst-case jitter (frames).
    pub ema_peak: f32,
    /// EWMA of jitter² for variance tracking.
    /// Combined with `ema_jitter`, yields coefficient of variation (CV = σ/μ)
    /// to distinguish stable-low-jitter from spiky-bursty networks.
    pub ema_jitter_var: f32,
    /// Consecutive packets with jitter below the adaptive clean threshold.
    /// Used to infer network quality: high streak = stable link.
    pub clean_streak: u32,
    /// Per-tick decay factor applied to `ema_peak`.
    pub ema_peak_decay_alpha: f32,
    /// When the last major jitter spike (>50ms) occurred.
    last_macro_spike: Option<Instant>,
    /// Unstable network (e.g. 2.4GHz scan cycle) regime expiration.
    unstable_regime_until: Option<Instant>,
    /// Last ingested sequence number to detect consecutive packets for IAT.
    last_ingest_seq: Option<u64>,
    /// Wall-clock arrival of the last forward packet, for IAT computation.
    last_network_arrival: Option<Instant>,
    // --- NetEQ Peak Detection State ---
    /// History of recent peaks (period_ms, height_frames).
    peak_history: VecDeque<(u64, f32)>,
    /// Time of the last detected peak.
    last_peak_time: Option<Instant>,
    /// Whether peak mode is currently active.
    peak_mode_active: bool,
    // --- NetEQ IAT Histogram (Method 1) ---
    /// 65-bin Q30 exponential-forgetting histogram of inter-arrival times.
    /// iat_histogram[i] = probability mass that IAT == i packet-periods (Q30).
    iat_histogram: [i64; 65],
    /// Forgetting-factor convergence counter. Starts at 0, converges toward
    /// `IAT_FACTOR_STEADY` (≈0.9993 in Q15) over the first ~1000 packets.
    iat_factor: i64,
    /// 95th-percentile IAT in frames, derived from the histogram each update.
    pub iat_percentile_target: f32,
}

impl JitterStats {
    pub fn new(config: &JitterConfig) -> Self {
        let halflife_ticks =
            (config.peak_decay_halflife_ms.max(10) as f32) / (MILLIS_PER_FRAME as f32);
        let ema_peak_decay_alpha = 0.5f32.powf(1.0 / halflife_ticks);
        let mut s = Self {
            ema_jitter: 0.0,
            ema_peak: 0.0,
            ema_jitter_var: 0.0,
            clean_streak: 0,
            ema_peak_decay_alpha,
            last_macro_spike: None,
            unstable_regime_until: None,
            last_ingest_seq: None,
            last_network_arrival: None,
            peak_history: VecDeque::with_capacity(8),
            last_peak_time: None,
            peak_mode_active: false,
            iat_histogram: [0i64; 65],
            iat_factor: 0,
            iat_percentile_target: 0.0,
        };
        s.reset_histogram_to_seed();
        s
    }

    /// Seed the IAT histogram with NetEQ's `ResetHistogram` distribution:
    /// bin[i] = (1<<30) * 0.5^(i+1) in Q30. This geometric distribution places
    /// the 95th-percentile at bin 4 (40ms), giving a stable 4-frame start target
    /// before any packets arrive — prevents the cold-start target collapse to 0.
    fn reset_histogram_to_seed(&mut self) {
        let mut val: i64 = 1 << 29; // 0.5 in Q30
        for bin in self.iat_histogram.iter_mut() {
            *bin = val;
            val >>= 1;
        }
        self.iat_factor = 0;
        self.iat_percentile_target = 4.0;
    }

    /// NetEQ `DelayManager::UpdateHistogram` + `CalculateTargetLevel` port.
    ///
    /// Feeds the observed inter-arrival time (in whole packet-periods) into a
    /// 65-bin Q30 exponential-forgetting histogram, then walks the reverse CDF to
    /// find the 95th-percentile IAT. Result is stored in `iat_percentile_target`
    /// (frames). Mirrors `delay_manager.cc:262-303`.
    fn update_iat_histogram(&mut self, iat_frames: f32) {
        /// Steady-state forgetting factor (≈0.9993 in Q15). ~1000-update half-life.
        const IAT_FACTOR_STEADY: i64 = 32745;
        /// 5% tail probability in Q30 → 95th percentile.
        const LIMIT_PROBABILITY: i64 = (1 << 30) / 20;

        let iat_packets = (iat_frames.max(0.0) as usize).min(64);

        // Exponential forgetting: scale every bin by iat_factor/32768.
        let mut vector_sum: i64 = 0;
        for bin in self.iat_histogram.iter_mut() {
            *bin = (*bin * self.iat_factor) >> 15;
            vector_sum += *bin;
        }
        // Bump the observed bin by (1 - iat_factor) in Q30.
        let increment = (32768 - self.iat_factor) << 15;
        self.iat_histogram[iat_packets] += increment;
        vector_sum += increment;

        // Correct rounding drift so the histogram sums to exactly 1.0 in Q30.
        vector_sum -= 1 << 30;
        if vector_sum != 0 {
            let flip = if vector_sum > 0 { -1i64 } else { 1i64 };
            for bin in self.iat_histogram.iter_mut() {
                if vector_sum == 0 {
                    break;
                }
                let correction = flip * vector_sum.abs().min(*bin >> 4);
                *bin += correction;
                vector_sum += correction;
            }
        }

        // Converge iat_factor toward steady state.
        self.iat_factor += (IAT_FACTOR_STEADY - self.iat_factor + 3) >> 2;

        // Reverse-CDF walk: accumulate from the tail until we cross the 5% limit.
        let mut sum: i64 = (1 << 30) - self.iat_histogram[0];
        let mut index = 0usize;
        while sum > LIMIT_PROBABILITY && index < 63 {
            index += 1;
            sum -= self.iat_histogram[index];
        }
        self.iat_percentile_target = index as f32;
    }

    /// Compute the stability ratio from the clean streak counter.
    /// Returns 0.0 (unstable) to 1.0 (highly stable).
    /// Ramps linearly over 400 consecutive clean packets (~2 seconds).
    pub fn stability_ratio(&self) -> f32 {
        self.clean_streak.min(400) as f32 / 400.0
    }

    /// Adaptive clean threshold based on observed baseline jitter.
    /// On 2.4GHz (ema_jitter ~4–8 frames), threshold ≈ 7–13 frames.
    /// On 5GHz (ema_jitter ~0.5 frames), threshold ≈ 1.75 frames.
    /// This allows the clean streak to build even on noisy networks,
    /// as long as jitter stays near the established baseline.
    fn clean_threshold(&self) -> f32 {
        (self.ema_jitter * 1.5 + 1.0).min(10.0)
    }

    /// Recompute the peak decay factor after a config change.
    pub fn recompute_decay_alpha(&mut self, config: &JitterConfig) {
        let halflife_ticks =
            (config.peak_decay_halflife_ms.max(10) as f32) / (MILLIS_PER_FRAME as f32);
        self.ema_peak_decay_alpha = 0.5f32.powf(1.0 / halflife_ticks);
    }

    /// Expose the unstable-regime deadline for the target controller's probe gate.
    pub fn unstable_regime_until(&self) -> Option<Instant> {
        self.unstable_regime_until
    }

    /// Whether the discrete NetEQ peak detector is currently active (≥2 peaks
    /// within the tracking window and not yet timed out).
    pub fn peak_mode_active(&self) -> bool {
        self.peak_mode_active
    }

    /// The maximum peak height (frames) currently recorded in the peak history.
    /// Only meaningful when `peak_mode_active()` is true. Unlike `ema_peak`, this
    /// drops to 0 the moment peak mode deactivates — no 20s lingering decay.
    pub fn peak_mode_height(&self) -> f32 {
        self.peak_history
            .iter()
            .map(|(_, h)| *h)
            .fold(0.0f32, f32::max)
    }

    /// Observe a single forward packet's arrival, updating all jitter statistics.
    ///
    /// `effective_target` is the controller's current locked-in depth, read by the
    /// NetEQ 2-peak state machine. Returns `true` if the caller should insert the
    /// packet into the reorder buffer, or `false` to drop it (clock ran backwards).
    pub fn observe(
        &mut self,
        seq_num: u64,
        arrival_time: Instant,
        config: &JitterConfig,
    ) -> bool {
        if let Some(last_time) = self.last_network_arrival
            && let Some(last_seq) = self.last_ingest_seq
            // Only compute for forward progress. Ignore reordered packets for jitter math.
            && seq_num > last_seq
        {
            let seq_diff = seq_num - last_seq;
            // If the gap is impossibly large (> 5 seconds), it's likely a complete stream resume.
            // We don't want to record 5000ms of jitter. Discard extreme anomalies.
            if seq_diff < 1000 {
                let iat_actual = match arrival_time.checked_duration_since(last_time) {
                    Some(d) => d.as_millis() as f32,
                    None => return false,
                };
                let iat_expected = (seq_diff as f32) * (MILLIS_PER_FRAME as f32);
                let jitter_ms = (iat_actual - iat_expected).max(0.0);
                let jitter_frames = jitter_ms / MILLIS_PER_FRAME as f32;

                // --- NetEQ IAT histogram update ---
                // Feed the actual inter-arrival time (in packet-times) into the Q30 histogram.
                // NetEQ model: record the full wall-clock gap, then subtract the missing-packet
                // count (seq_diff - 1). For consecutive packets seq_diff==1, so the raw gap is
                // recorded directly — capturing gap-then-burst spikes that the old divide-by-seq_diff
                // normalization washed out.
                let iat_packets = iat_actual / MILLIS_PER_FRAME as f32;
                let iat_adjusted = (iat_packets - (seq_diff as f32 - 1.0)).max(0.0);
                self.update_iat_histogram(iat_adjusted);

                // --- Clean streak tracking (regime-aware) ---
                // A packet is "clean" if its jitter is below the adaptive threshold.
                // On 2.4GHz where baseline jitter is ~20ms, the threshold rises to
                // accommodate the network's natural behavior, letting the streak build.
                let clean_thresh = self.clean_threshold();
                if jitter_frames < clean_thresh {
                    self.clean_streak = self.clean_streak.saturating_add(1);
                } else {
                    // Spikes well above threshold are severe — slam to zero.
                    // Moderate spikes (up to 2x threshold) decay gently.
                    if jitter_frames > clean_thresh * 2.0 {
                        self.clean_streak = 0;
                    } else {
                        self.clean_streak /= 2;
                    }
                }

                // --- Jitter variance tracking (EWMA of jitter²) ---
                let jitter_sq = jitter_frames * jitter_frames;
                self.ema_jitter_var = self.ema_jitter_var * 0.95 + jitter_sq * 0.05;

                // --- Stability-aware jitter EMA decay ---
                // Fast attack (α=0.15) on spikes, stability-scaled decay on clean packets.
                // Stable 5GHz: α_decay ≈ 0.04 → halves in ~85 callbacks (~425ms)
                // Unstable 2.4GHz: α_decay ≈ 0.005 → halves in ~700 callbacks (~3.5s)
                let stability = self.stability_ratio();
                let alpha = if jitter_frames > self.ema_jitter {
                    0.15 // Fast attack
                } else {
                    lerp(0.005, 0.04, stability)
                };
                self.ema_jitter = self.ema_jitter * (1.0 - alpha) + jitter_frames * alpha;

                // --- Peak decay: stability-aware continuous interpolation ---
                let mut current_decay_alpha = self.ema_peak_decay_alpha;
                if config.peak_decay_halflife_ms == 0 {
                    // Smart Mode (Auto): interpolate half-life based on stability.
                    // Stable network: 1.5s half-life (aggressive shedding)
                    // Unstable network: 34.6s half-life (cautious retention)
                    let is_unstable = self
                        .unstable_regime_until
                        .is_some_and(|unstable_until| arrival_time < unstable_until);

                    let halflife_ms = if is_unstable {
                        // In unstable regime, clamp to slow decay regardless of streak
                        // Reduced from 34.6s: still cautious but recovers in seconds, not minutes
                        15000.0
                    } else {
                        // Continuous interpolation: 10s → 0.8s based on stability
                        // Compressed from 34.6s/1.5s: enables recovery from isolated
                        // spikes within seconds instead of minutes
                        lerp(10000.0, 800.0, stability)
                    };
                    let halflife_ticks = halflife_ms / MILLIS_PER_FRAME as f32;
                    current_decay_alpha = 0.5f32.powf(1.0 / halflife_ticks);

                    // Track spikes > 50ms (10 frames)
                    if jitter_frames >= 10.0 {
                        let mut is_new_macro_spike = false;
                        if let Some(last_spike) = self.last_macro_spike {
                            let interval = arrival_time.duration_since(last_spike).as_millis();
                            if interval > 500 {
                                // Debounce burst packets
                                is_new_macro_spike = true;
                                // If spikes are frequent (<10s), network is chronically poor
                                if interval < 10000 {
                                    // Reduced from 60s: 20s is long enough to absorb
                                    // a burst of spikes without permanently locking
                                    // the target at elevated levels
                                    self.unstable_regime_until =
                                        Some(arrival_time + Duration::from_secs(20));
                                }
                            }
                        } else {
                            is_new_macro_spike = true;
                        }
                        if is_new_macro_spike {
                            self.last_macro_spike = Some(arrival_time);
                        }
                    }
                }

                // --- NetEQ 2-Peak Trigger State Machine (Method 6) ---
                // A peak is a delay spike that exceeds the target + threshold (approx 3 frames).
                // If we see 2 peaks within a 10s window, we lock the peak height as the target.
                // Use the histogram base (NetEQ base_target_level) as the peak threshold,
                // NOT the peak-inflated effective_target. Using effective_target creates a
                // feedback loop: peaks raise the target, which raises the threshold, which
                // stops peaks from registering, which collapses the target — oscillation.
                let target_level = self.iat_percentile_target;
                let threshold = 3.9; // 78ms at 20ms/frame
                if jitter_frames > target_level + threshold || jitter_frames > 2.0 * target_level {
                    if let Some(last) = self.last_peak_time {
                        let period_ms = arrival_time.duration_since(last).as_millis() as u64;
                        if period_ms <= 10000 {
                            self.peak_history.push_back((period_ms, jitter_frames));
                            if self.peak_history.len() > 8 {
                                self.peak_history.pop_front();
                            }
                            self.last_peak_time = Some(arrival_time);
                        } else if period_ms <= 20000 {
                            self.last_peak_time = Some(arrival_time);
                        } else {
                            self.peak_history.clear();
                            self.last_peak_time = Some(arrival_time);
                        }
                    } else {
                        self.last_peak_time = Some(arrival_time);
                    }
                }

                if self.peak_history.len() >= 2 {
                    if let Some(last) = self.last_peak_time {
                        let max_period =
                            self.peak_history.iter().map(|(p, _)| *p).max().unwrap_or(1);
                        let elapsed = arrival_time.duration_since(last).as_millis() as u64;
                        self.peak_mode_active = elapsed <= 2 * max_period;
                    }
                } else {
                    self.peak_mode_active = false;
                }

                // ema_peak: slow-decay peak tracker.
                // Instead of jumping on every single spurious frame, it only jumps when
                // the NetEQ Peak State Machine verifies a recurring delay spike.
                self.ema_peak *= current_decay_alpha;
                if self.peak_mode_active {
                    let max_peak = self
                        .peak_history
                        .iter()
                        .map(|(_, h)| *h)
                        .fold(0.0f32, |a, b| a.max(b));
                    self.ema_peak = self.ema_peak.max(max_peak);
                }
            }
        }
        self.last_network_arrival = Some(arrival_time);
        self.last_ingest_seq = Some(seq_num);
        true
    }

    /// Partial reset on stream restart (matches the legacy `trigger_reset` field set):
    /// clears the jitter EMAs and streak but preserves the peak-detection history and
    /// the last network-arrival anchor.
    pub fn reset_on_stream_restart(&mut self) {
        self.ema_jitter = 0.0;
        self.ema_peak = 0.0;
        self.last_ingest_seq = None;
        self.clean_streak = 0;
        self.ema_jitter_var = 0.0;
        self.reset_histogram_to_seed();
    }

    pub fn reset_on_config_change(&mut self) {
        self.ema_jitter = 0.0;
        self.ema_peak = 0.0;
        self.clean_streak = 0;
        self.ema_jitter_var = 0.0;
        self.peak_history.clear();
        self.last_peak_time = None;
        self.peak_mode_active = false;
        self.reset_histogram_to_seed();
    }
}
