//! Depth-decision actor: owns the adaptive target-depth policy — hysteresis,
//! quantization, rate-limited ramping, downward probing, and the post-starvation
//! bump. Reads [`super::stats::JitterStats`] and [`JitterConfig`] as inputs; owns
//! no buffer or decoder. The orchestrator feeds it the raw computed target each
//! callback and receives the smoothed effective target back.

use std::time::Instant;

use super::consts::ms_to_frames_ceil;
use super::stats::JitterStats;
use crate::domain::types::JitterConfig;

/// Hysteresis half-width in frames. The effective target only moves when the
/// raw computed target deviates by more than this many frames.
const HYSTERESIS_BAND: u32 = 3;
/// Snap effective target to multiples of this many frames to reduce
/// the total number of discrete target transitions.
const TARGET_QUANTUM: u32 = 4;
/// Rate-limit interval: effective target moves by at most ±1 frame every
/// this many callbacks, smoothing transitions for artifact-free playback.
const RAMP_INTERVAL: u32 = 5;
/// When the network has been stable for a sustained period, try probing
/// lower every this many callbacks. One quantum step down per probe.
const PROBE_DOWN_INTERVAL: u32 = 200;

/// Adaptive target-depth controller. Holds all hysteresis, ramp, probe, and
/// starvation-bump state; produces the smoothed effective target each callback.
pub(super) struct TargetController {
    /// The currently locked-in effective target depth (frames). Only moves when
    /// the raw computed target exits the hysteresis band for the dwell period.
    pub effective_target: u32,
    /// How many consecutive callbacks the raw target has been outside the band.
    pub target_exit_count: u32,
    /// The quantized goal that `effective_target` is ramping toward.
    pub ramp_goal: u32,
    /// Countdown for rate-limited ramping (one step per RAMP_INTERVAL callbacks).
    ramp_countdown: u32,
    /// Countdown for active downward probing when the network is stable.
    probe_down_countdown: u32,
    /// Learned floor: the lowest effective_target that caused starvation.
    /// Probing won't go below this. Reset when network conditions genuinely change.
    probe_floor: u32,
}

impl TargetController {
    pub fn new() -> Self {
        Self {
            effective_target: 2,
            target_exit_count: 0,
            ramp_goal: 2,
            ramp_countdown: 0,
            probe_down_countdown: PROBE_DOWN_INTERVAL,
            probe_floor: 0,
        }
    }

    /// Minimum buffer depth in frames, from config.
    fn min_depth_frames(config: &JitterConfig) -> u32 {
        ms_to_frames_ceil(config.min_depth_ms)
    }

    /// Comfort cap in frames, from config.
    fn comfort_cap_frames(config: &JitterConfig) -> f32 {
        ms_to_frames_ceil(config.comfort_cap_ms) as f32
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
    fn adaptive_quantum(config: &JitterConfig) -> u32 {
        let cap = Self::comfort_cap_frames(config) as u32;
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
    fn adaptive_hysteresis(config: &JitterConfig) -> u32 {
        let cap = Self::comfort_cap_frames(config) as u32;
        if cap <= 8 { 1 } else { HYSTERESIS_BAND }
    }

    /// Adaptive dwell time: how many callbacks the raw target must stay
    /// outside the hysteresis band before committing to a new goal.
    /// High-cap presets (Auto, Resilient) react faster since they have
    /// more headroom. Low-cap presets stay conservative.
    fn adaptive_dwell(config: &JitterConfig) -> u32 {
        let cap = Self::comfort_cap_frames(config) as u32;
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
    pub fn compute_target_depth(
        &self,
        config: &JitterConfig,
        stats: &JitterStats,
        tcp_cap_override: Option<f32>,
    ) -> u32 {
        // Static mode: lock buffer to exact user-specified depth, bypass all adaptive math.
        if let Some(static_ms) = config.static_target_ms {
            return ms_to_frames_ceil(static_ms).max(Self::min_depth_frames(config));
        }
        let stability = stats.stability_ratio();
        let margin_scale = 1.0 - stability * 0.4;
        // Use histogram 95th-percentile as the base target. Add discrete peak
        // height only when peak mode is active (NetEQ DelayPeakDetector style):
        // binary on/off, drops to zero the moment peak mode deactivates.
        // This prevents ema_peak from holding the target high for ~20s after
        // the network calms — the root cause of the oscillation in log 2.
        let histogram_base = stats.iat_percentile_target;
        // NetEQ combines the histogram base and the peak height via MAX, not addition
        // (delay_manager.cc:294 `std::max(target_level, MaxPeakHeight())`). The peak
        // overrides the base only when it's higher; the two are never summed.
        let peak_target = if stats.peak_mode_active() {
            stats.peak_mode_height() * margin_scale
        } else {
            0.0
        };
        let target = (Self::min_depth_frames(config) as f32)
            .max(histogram_base)
            .max(peak_target);
        let cap = tcp_cap_override.unwrap_or(Self::comfort_cap_frames(config));
        let safe_cap = cap.max(Self::min_depth_frames(config) as f32);
        target
            .ceil()
            .clamp(Self::min_depth_frames(config) as f32, safe_cap) as u32
    }


    /// NetEQ `DelayManager::BufferLimits` — the drain decision band around the
    /// effective target, in whole frames. The orchestrator compares the filtered
    /// buffer level against these:
    ///   - `filtered >= 4 * high`  → emergency drain (fast accelerate, no cooldown)
    ///   - `filtered >= high`      → gentle drain (normal accelerate, cooldown)
    ///   - `filtered <  low`       → preemptive expand (slow down)
    ///
    /// `low = 0.75 * target`; `high = max(target, low + MIN_BAND)`. The minimum
    /// spread guarantees a dead-band between the expand and accelerate decisions so
    /// we don't ping-pong between them. NetEQ uses a 20ms window; we widen it to
    /// 40ms because on jittery links (2.4GHz) the filtered level micro-oscillates by
    /// 60-100ms, and a 20ms band let every small swing cross a limit and fire a
    /// splice per oscillation — the per-oscillation stutter. A 40ms dead-band keeps
    /// those micro-swings inside it; only genuine excursions cross. Mirrors
    /// `delay_manager.cc:358-375` (adapted from Q8 packets to whole frames).
    pub fn buffer_limits(target: u32) -> (u32, u32) {
        /// Minimum low→high spread in frames (40ms / MILLIS_PER_FRAME). Wider than
        /// NetEQ's 20ms to absorb 2.4GHz micro-oscillation without triggering drain.
        const MIN_BAND: u32 = 40 / super::consts::MILLIS_PER_FRAME;
        let low = target * 3 / 4;
        let high = target.max(low + MIN_BAND);
        (low, high)
    }

    /// Reset hysteresis + ramp + probe state for a new config. Returns the new
    /// effective target so the orchestrator can compute its flush target.
    pub fn reset_for_config(&mut self, config: &JitterConfig) -> u32 {
        self.effective_target = ms_to_frames_ceil(config.min_depth_ms).max(2);
        self.ramp_goal = self.effective_target;
        self.target_exit_count = 0;
        self.ramp_countdown = 0;
        self.probe_down_countdown = PROBE_DOWN_INTERVAL;
        self.probe_floor = 0;
        self.effective_target
    }

    /// Full reset on stream restart.
    pub fn reset(&mut self) {
        self.effective_target = 2;
        self.ramp_goal = 2;
        self.target_exit_count = 0;
        self.ramp_countdown = 0;
        self.probe_down_countdown = PROBE_DOWN_INTERVAL;
        self.probe_floor = 0;
    }

    /// Apply hysteresis, quantization, rate-limited ramping, and downward probing
    /// to the raw computed target. Returns the smoothed effective target.
    ///
    /// `raw_target` is the orchestrator's current `compute_target_depth` result
    /// (already tcp-capped / static-overridden). `min_depth` is the config floor.
    pub fn advance(
        &mut self,
        config: &JitterConfig,
        stats: &JitterStats,
        raw_target: u32,
        min_depth: u32,
    ) -> u32 {
        // Static and No Buffer modes bypass hysteresis entirely.
        if config.static_target_ms.is_some() {
            self.effective_target = raw_target;
            self.ramp_goal = raw_target;
            return raw_target;
        }

        let quantum = Self::adaptive_quantum(config);
        let hysteresis = Self::adaptive_hysteresis(config);
        let quantized = Self::quantize_target(raw_target, quantum).max(min_depth);
        let diff = self.effective_target.abs_diff(quantized);

        if diff <= hysteresis {
            // Inside the dead-zone: no change, reset dwell counter.
            self.target_exit_count = 0;
        } else {
            self.target_exit_count += 1;
            if self.target_exit_count >= Self::adaptive_dwell(config) {
                // Sustained deviation — commit to new ramp goal.
                tracing::debug!(
                    "[JitterMgr] Target transition: effective={}→ramp_goal={}, raw={}, ema_jitter={:.2}, ema_peak={:.2}, stability={:.2}",
                    self.effective_target,
                    quantized,
                    raw_target,
                    stats.ema_jitter,
                    stats.ema_peak,
                    stats.stability_ratio(),
                );
                self.ramp_goal = quantized;
                self.target_exit_count = 0;
                // If the target is moving UP due to genuine network worsening, reset
                // probe floor so future probing can re-discover the new optimal depth.
                // Don't reset when the upward move is bump-driven — that would allow
                // re-probing back to the level that just caused starvation.
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
        } else if stats.stability_ratio() > 0.2
            && self.effective_target > min_depth
            // Allow probing even during unstable regime if current stability
            // is locally high enough — the regime lock is a coarse heuristic
            // and the probe_floor prevents re-probing below safe levels.
            && !stats
                .unstable_regime_until()
                .is_some_and(|until| Instant::now() < until && stats.stability_ratio() < 0.5)
        {
            // Active downward probing: when the network has been calm for
            // a sustained period, nudge the target down to discover the
            // lowest stable depth. Speed scales with confidence.
            let probe_interval = if stats.stability_ratio() > 0.8 {
                60 // High confidence: probe every ~300ms
            } else {
                120 // Normal: probe every ~600ms
            };
            self.probe_down_countdown = self.probe_down_countdown.saturating_sub(1);
            if self.probe_down_countdown == 0 {
                self.probe_down_countdown = probe_interval;
                let probe_goal =
                    Self::quantize_target(self.effective_target.saturating_sub(quantum), quantum)
                        .max(min_depth)
                        .max(self.probe_floor);
                if probe_goal < self.effective_target {
                    tracing::debug!(
                        "[JitterMgr] Probe down: effective={}→probe_goal={}, floor={}, stability={:.2}",
                        self.effective_target,
                        probe_goal,
                        self.probe_floor,
                        stats.stability_ratio(),
                    );
                    self.ramp_goal = probe_goal;
                }
            }
        }
        self.effective_target
    }

    /// Apply post-starvation probe floor update. Called by the orchestrator when
    /// the buffer recovers after a starvation event. Sets `probe_floor` so future
    /// probing won't descend back to the level that just caused starvation.
    pub fn apply_starvation_floor(
        &mut self,
        config: &JitterConfig,
        stats: &JitterStats,
    ) {
        let quantum = Self::adaptive_quantum(config);
        let dynamic_floor = self.compute_target_depth(config, stats, None);
        self.probe_floor = self
            .probe_floor
            .max(dynamic_floor)
            .max(self.effective_target.saturating_add(quantum));
        tracing::info!(
            "[JitterMgr] Starvation floor set: probe_floor={}, effective_target={}, ema_jitter={:.2}",
            self.probe_floor,
            self.effective_target,
            stats.ema_jitter,
        );
    }
}
