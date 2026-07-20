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
/// Cooldown period in callbacks after a starvation bump. While active,
/// no new bumps are applied — prevents positive-feedback ratcheting.
const STARVATION_COOLDOWN: u32 = 200;
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
    /// Additive target bump after starvation, bleeds continuously.
    pub starvation_bump: f32,
    /// Cooldown countdown after a starvation bump. While >0, no new bumps are applied.
    starvation_bump_cooldown: u32,
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
            starvation_bump: 0.0,
            starvation_bump_cooldown: 0,
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
        // When the network is demonstrably stable (high clean_streak), reduce the
        // jitter_margin contribution so the target converges to min_depth faster.
        let stability = stats.stability_ratio();
        let margin_scale = 1.0 - stability * 0.4; // At full stability: 60% of raw margin
        let jitter_margin = (stats.ema_jitter * 2.0 + stats.ema_peak) * margin_scale;
        // Target is natively built on top of the user's requested minimum floor.
        // We do not add artificial hardcoded safety margins here.
        let target = Self::min_depth_frames(config) as f32 + jitter_margin + self.starvation_bump;
        let cap = tcp_cap_override.unwrap_or(Self::comfort_cap_frames(config));
        let safe_cap = cap.max(Self::min_depth_frames(config) as f32);
        target
            .ceil()
            .clamp(Self::min_depth_frames(config) as f32, safe_cap) as u32
    }

    /// Per-callback bleed of the starvation bump and cooldown tick.
    pub fn tick_bleed(&mut self) {
        // Proportional bleed for starvation bump: bigger bumps recover faster.
        // Increased rate from 0.05+3% to 0.08+5% — recovers ~40% faster.
        // 8-frame bump: bleeds at ~0.48 frames/cb → recovers in ~17 callbacks (85ms)
        // 2-frame bump: bleeds at ~0.18 frames/cb → recovers in ~11 callbacks (55ms)
        let bleed = 0.08 + self.starvation_bump * 0.05;
        self.starvation_bump = (self.starvation_bump - bleed).max(0.0);
        // Tick starvation bump cooldown.
        self.starvation_bump_cooldown = self.starvation_bump_cooldown.saturating_sub(1);
    }

    /// Reset hysteresis + ramp + probe state for a new config. Returns the new
    /// effective target so the orchestrator can compute its flush target.
    pub fn reset_for_config(&mut self, config: &JitterConfig) -> u32 {
        self.starvation_bump = 0.0;
        self.effective_target = ms_to_frames_ceil(config.min_depth_ms).max(2);
        self.ramp_goal = self.effective_target;
        self.target_exit_count = 0;
        self.ramp_countdown = 0;
        self.starvation_bump_cooldown = 0;
        self.probe_down_countdown = PROBE_DOWN_INTERVAL;
        self.probe_floor = 0;
        self.effective_target
    }

    /// Full reset on stream restart.
    pub fn reset(&mut self) {
        self.starvation_bump = 0.0;
        self.effective_target = 2;
        self.ramp_goal = 2;
        self.target_exit_count = 0;
        self.ramp_countdown = 0;
        self.starvation_bump_cooldown = 0;
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
        } else if stats.stability_ratio() > 0.2
            && self.effective_target > min_depth
            && self.starvation_bump < 0.5
            && self.starvation_bump_cooldown == 0
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

    /// Apply the post-starvation bump. Called by the orchestrator when the buffer
    /// recovers after a starvation event (and the cooldown has expired). Reads
    /// `starvation_count` for logging only.
    pub fn apply_starvation_bump(
        &mut self,
        config: &JitterConfig,
        stats: &JitterStats,
        starvation_count: u32,
    ) {
        if self.starvation_bump_cooldown != 0 {
            return;
        }
        // Differentiate probe-induced starvation from genuine network outage.
        // If we were recently stable (probing), use a mild bump that just
        // returns to the previous level. If genuinely unstable, use full bump.
        let is_probe_failure = stats.stability_ratio() > 0.3;
        if is_probe_failure {
            let quantum = Self::adaptive_quantum(config);
            // Minimal bump: just 1 frame. The probe_floor mechanism
            // prevents re-probing below this level, so we don't need
            // a large bump to stay safe.
            let mild_bump = 1.0;
            self.starvation_bump = self.starvation_bump.max(mild_bump);
            // Set floor using the full dynamic formula, not just +1 quantum.
            // This prevents repeated starvation on bad networks: if ema_peak
            // is 20 frames (100ms), floor jumps to ~200ms immediately.
            let dynamic_floor = self.compute_target_depth(config, stats, None);
            self.probe_floor = self
                .probe_floor
                .max(dynamic_floor)
                .max(self.effective_target.saturating_add(quantum));
        } else {
            // Genuine starvation: use ema_jitter (stable estimate) instead
            // of ema_peak (spike-driven, can be vastly inflated). Cap at 8.
            let bump = (stats.ema_jitter * 2.0 + 2.0).min(8.0);
            self.starvation_bump = self.starvation_bump.max(bump);
        }
        tracing::info!(
            "[JitterMgr] Starvation bump: type={}, bump={:.1}, effective_target={}, ema_jitter={:.2}, ema_peak={:.2}, starvation_frames={}",
            if is_probe_failure { "probe" } else { "genuine" },
            self.starvation_bump,
            self.effective_target,
            stats.ema_jitter,
            stats.ema_peak,
            starvation_count,
        );
        self.starvation_bump_cooldown = STARVATION_COOLDOWN;
        // Upward bump bypasses hysteresis dwell for immediate safety.
        let boosted = Self::quantize_target(
            self.compute_target_depth(config, stats, None),
            Self::adaptive_quantum(config),
        )
        .max(Self::min_depth_frames(config));
        if boosted > self.effective_target {
            self.ramp_goal = boosted;
            self.target_exit_count = 0;
        }
    }
}
