//! Depth-decision actor: owns the adaptive target-depth policy — hysteresis,
//! quantization, rate-limited ramping, downward probing, and the post-starvation
//! bump. Reads [`super::stats::JitterStats`] and [`JitterConfig`] as inputs; owns
//! no buffer or decoder. The orchestrator feeds it the raw computed target each
//! callback and receives the smoothed effective target back.

use std::collections::VecDeque;
use std::time::Instant;

use super::consts::{MILLIS_PER_FRAME, ms_to_frames_ceil};
use super::stats::JitterStats;
use crate::domain::types::JitterConfig;

/// Hysteresis half-width in frames. The effective target only moves when the
/// raw computed target deviates by more than this many frames.
const HYSTERESIS_BAND: u32 = 3;
/// Snap effective target to multiples of this many frames to reduce
/// the total number of discrete target transitions.
const TARGET_QUANTUM: u32 = 4;
/// Rate-limit interval: effective target moves DOWN by at most 1 frame every
/// this many callbacks, smoothing transitions for artifact-free playback.
const RAMP_INTERVAL: u32 = 5;
/// Frames per step when climbing, and callbacks between steps: ~100 frames/s of
/// target growth. Faster than any real link degradation, yet slow enough that
/// `low_limit` never outruns the actual buffer by more than a frame or two.
///
/// The old behaviour — jump straight to `ramp_goal` in one callback — is what
/// produced the "fast clicking on every buffer increase": a 15-frame jump puts
/// `low_limit` 11 frames above the real buffer level, and the preemptive-expand
/// path then fires a single-pitch-period OLA splice every cooldown window for
/// several seconds straight.
const RAMP_UP_STEP: u32 = 2;
const RAMP_INTERVAL_UP: u32 = 2;
/// When the network has been stable for a sustained period, try probing
/// lower every this many callbacks. One quantum step down per probe.
const PROBE_DOWN_INTERVAL: u32 = 200;

/// How long the quantized target must stay *below* the hysteresis band before a
/// downward transition commits. Upward transitions keep `adaptive_dwell`
/// (15-20 callbacks) untouched — a worsening link must still be answered in
/// ~150-200ms, and nothing here may slow that down.
///
/// **Why the asymmetry exists.** `gap_floor = 1.25 * max_gap + 1`, so one
/// `GAP_STALE_DECAY` step past the flat-top moves the raw target
/// by `Δ = 1.25 * max_gap * 0.15 = 0.1875 * max_gap`, against a dead-zone of
/// `adaptive_hysteresis = (cap/8).clamp(1,3)` — **3 frames on every Auto link**.
/// Break-even is therefore `max_gap = 3 / 0.1875 = 16 frames = 160ms`, and the
/// four-link capture set lands on either side of it:
///
/// | capture | mean `max_gap` | Δ per decay step | Δ/hysteresis | windows where one step clears the band |
/// | --- | --- | --- | --- | --- |
/// | 5GHz uncompressed | 12.0 | 2.25 | 0.75 | 38% |
/// | 5GHz 128kbps | 8.4 | 1.58 | 0.52 | 26% |
/// | 2.4GHz uncompressed | 22.2 | 4.16 | **1.39** | **81%** |
/// | 2.4GHz 128kbps | 24.3 | 4.56 | **1.52** | **84%** |
///
/// On 2.4GHz a *single* decay step clears the dead-zone in 4 windows out of 5, so
/// the 150ms dwell is satisfied by ageing alone and the target commits downward on
/// no new evidence at all. Measured consequence: target descents are 14.9%/14.9%
/// of stream time but carry **66.0%/75.4%** of every accelerate splice (2.4x/2.9x
/// density), 60%/75% of them reverse within 20s, and the net drift between
/// consecutive large gaps is 2.4/3.8 frames against 400/310 frames actually shed —
/// oscillation, not convergence. 5GHz descents of ≥5 frames reverse **0%** of the
/// time; every 2.4GHz size bucket reverses 50-86%.
///
/// **Two independent derivations put the value at ~2s.** *Amplitude:* requiring 2s
/// means the decay must run twice before the target moves, i.e. the evidence must
/// be roughly twice the dead-zone — restoring on 2.4GHz the margin 5GHz already
/// gets for free. *Timing:* `raw_target` first falls when the gap window's flat-top
/// expires at `GAP_FRESH_SECS` = 8s, so +2s puts the commit at 10s
/// from the last gap, against a measured age-reset period of p50 = 5.4-6.6s and
/// p75 = 8.8-10.0s — the next gap has already re-armed the target before the
/// descent commits in ~78% of periods, against ~70% today.
///
/// Modelled by replaying the logged `raw_target` and `probe_floor` through
/// `advance()` (only the 2.4GHz rows are quotable — the 1Hz replay reproduces
/// 24-unc within 3% but 5G-unc only within 40%):
///
/// | capture | down transitions | frames shed | accelerates vs today | mean latency |
/// | --- | --- | --- | --- | --- |
/// | 24-unc, today | 88 | 412 | 103% | −0.9ms |
/// | 24-unc, 2s | **45 (−49%)** | 348 | **87%** | +7.9ms |
/// | 24-128k, today | 59 | 312 | 101% | +0.5ms |
/// | 24-128k, 2s | **28 (−53%)** | 252 | **81%** | +10.8ms |
///
/// Halving the transitions while cutting drain splices only 13-19% is the honest
/// reading: the dwell removes the *oscillation* and merely delays the descents that
/// are real. 5GHz's large descents are genuine and still complete, 2s later.
///
/// **NetEQ has no dwell at all** — its target is one exponential with
/// `forget_factor = 32745/32768` ([delay_manager.cc:74](delay_manager.cc#L74)),
/// τ = 1/(1−f) = 1425 packets = **14.25s**, symmetric and slow in both directions.
/// Our asymmetry exists precisely because we also carry an explicit `max_gap` term
/// that must be free to rise within a single packet.
///
/// Counted in callbacks rather than wall-clock, matching the up dwell it sits
/// beside. Under a stalled callback thread (measured `window_ms > 1500` in 4.0% of
/// 24-unc windows) 200 callbacks span *more* than 2s, which holds the target up
/// longer exactly when the device is struggling — the safe direction.
const DOWN_DWELL_MS: u32 = 2000;

/// How long to track starvation events for rate limiting probes.
const STARVATION_WINDOW_SECS: u64 = 30;
/// Probe interval when starvation occurred recently — 6 seconds between
/// probe steps instead of the normal 300-600ms. Prevents the
/// probe-down→starve→bump-up→probe-down oscillation.
const PROBE_GATED_INTERVAL: u32 = 600;

/// Starvation-free seconds before the learned floor begins to relax.
const FLOOR_HOLD_SECS: u64 = 8;
/// Seconds between successive one-quantum relaxations of the learned floor.
/// At 2s a floor sitting at the `cap/2` ceiling (40 frames on 2.4GHz) is fully
/// relaxed in 8 + 10×2 ≈ 28s — inside the screen-on snap-back budget. At the
/// previous 4s it took ~48s, which alone blew that budget.
const FLOOR_DECAY_SECS: u64 = 2;

/// Which term of the `.max()` chain in [`TargetController::target_breakdown`]
/// produced the raw target.
///
/// Exists because three consecutive rounds of field diagnosis had to *infer* the
/// winning term by hand-arithmetic from `max_gap` and the link profile, and got it
/// wrong more than once — a 5GHz starvation storm was blamed on a `probe_floor`
/// ratchet that the same log recorded as 3. The term is a fact the code already
/// knows and was throwing away.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TargetTerm {
    /// The config floor — nothing observed justified any depth.
    MinDepth,
    /// p95 of the relative-arrival-delay histogram (NetEQ's base target).
    Histogram,
    /// Worst recent delivery gap + headroom.
    GapFloor,
    /// Static / no-buffer preset: the user pinned the depth, no adaptive term ran.
    Static,
}

impl TargetTerm {
    /// Stable short name for logs. `&'static str` so the hot-path log site never
    /// allocates.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::MinDepth => "min_depth",
            Self::Histogram => "histogram",
            Self::GapFloor => "gap_floor",
            Self::Static => "static",
        }
    }
}

/// Every input to one depth decision, plus which term won it.
///
/// Returned by [`TargetController::target_breakdown`], which is the single place the
/// depth arithmetic lives — the decision and the log read the same struct, so an
/// observability line can never drift from the behaviour it reports.
pub(super) struct TargetBreakdown {
    pub histogram_base: f32,
    /// **Reported, not authoritative.** `inter_burst_gap + 2` — what the removed
    /// burst term *would* have demanded. Kept in the log so the next capture can
    /// confirm it stays dead, and so the divergence from `gap_floor` (one ADB
    /// capture: 5783.5 vs a `max_gap` of 24) stays visible instead of silently
    /// returning.
    pub burst_floor: f32,
    pub gap_floor: f32,
    pub min_depth: f32,
    /// Pre-clamp maximum of the chain. Differs from `raw` exactly when the comfort
    /// cap (or the min-depth floor) bound the result — the 5GHz storm's saturated
    /// histogram reads 63 here and 40 in `raw`, and only the pair distinguishes
    /// "the link is that bad" from "the statistic is pinned at its ceiling".
    pub pre_clamp: f32,
    pub winning: TargetTerm,
    pub raw: u32,
}

/// The drain/expand decision band around the effective target, in whole frames.
///
/// Named rather than a `(u32, u32)` because the two ends drive opposite actuators
/// and the resume clamp reads only `high` — a positional `.1` there is the single
/// most consequential read in the module, and picking the wrong element is silent.
/// Produced only by [`TargetController::buffer_limits`], whose doc states the
/// arithmetic and the NetEQ correspondence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Band {
    /// `filtered < low` → preemptive expand.
    pub low: u32,
    /// `filtered >= high` → gentle drain. Monotone non-decreasing in `target`,
    /// pinned by `the_high_limit_must_never_fall_as_the_target_rises`.
    pub high: u32,
}

/// Adaptive target-depth controller. Holds all hysteresis, ramp, probe, and
/// starvation-bump state; produces the smoothed effective target each callback.
pub(super) struct TargetController {
    /// The currently locked-in effective target depth (frames). Only moves when
    /// the raw computed target exits the hysteresis band for the dwell period.
    pub effective_target: u32,
    /// How many consecutive callbacks the raw target has been outside the band.
    pub target_exit_count: u32,
    /// Which way the pending transition points. A reversal restarts the dwell
    /// rather than inheriting the opposite direction's evidence — without it, 199
    /// callbacks of falling evidence would let a single rising callback commit
    /// immediately, and vice versa. Only meaningful while `target_exit_count > 0`,
    /// which is why `advance` refreshes it on every restart and the three external
    /// reset paths do not have to.
    exit_falling: bool,
    /// The quantized goal that `effective_target` is ramping toward.
    pub ramp_goal: u32,
    /// Countdown for rate-limited ramping (one step per RAMP_INTERVAL callbacks).
    ramp_countdown: u32,
    /// Countdown for active downward probing when the network is stable.
    probe_down_countdown: u32,
    /// Learned floor: the lowest effective_target that caused starvation.
    /// Probing won't go below this. Bounded at `cap/2` and relaxed on a timer,
    /// so it can never be the thing that pins the target at the comfort cap.
    probe_floor: u32,
    /// Recent starvation timestamps for rate-limiting probe aggressiveness.
    starvation_timestamps: VecDeque<Instant>,
    /// When the most recent starvation was recorded, for the floor-relax hold.
    last_starvation: Option<Instant>,
    /// When the floor was last relaxed by one quantum.
    last_floor_decay: Instant,
}

impl TargetController {
    pub fn new() -> Self {
        Self {
            effective_target: 2,
            target_exit_count: 0,
            exit_falling: false,
            ramp_goal: 2,
            ramp_countdown: 0,
            probe_down_countdown: PROBE_DOWN_INTERVAL,
            probe_floor: 0,
            starvation_timestamps: VecDeque::new(),
            last_starvation: None,
            last_floor_decay: Instant::now(),
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

    /// The learned starvation floor, in frames. Read-only: the floor may only be
    /// raised by `apply_starvation_floor` and lowered by its own timed decay, so
    /// exposing it for logging (and for asserting "one gap costs one floor bump")
    /// must not hand out a way to set it.
    pub fn probe_floor(&self) -> u32 {
        self.probe_floor
    }

    /// Snap a raw target to the nearest quantum level.
    /// Reduces the total number of discrete target transitions.
    fn quantize_target(raw: u32, quantum: u32) -> u32 {
        if quantum == 0 {
            return raw;
        }
        ((raw + quantum / 2) / quantum) * quantum
    }

    /// Adaptive quantum: step size for target transitions, scaled to the comfort
    /// cap so a preset always has ~16 discrete levels to settle on.
    ///
    /// Derived from the cap rather than branching on thresholds: the old
    /// `cap <= 8 → 1 / cap >= 100 → 2 / else 4` ladder was **non-monotone**, so
    /// an 800ms cap got coarser steps (4) than a 1000ms cap (2).
    fn adaptive_quantum(config: &JitterConfig) -> u32 {
        let cap = Self::comfort_cap_frames(config) as u32;
        (cap / 16).clamp(1, TARGET_QUANTUM)
    }

    /// Adaptive hysteresis: dead-zone half-width, scaled to the comfort cap.
    /// Narrow for low-cap presets so the target can distinguish between 1, 2 and
    /// 3 frames instead of treating them all as within the dead-zone.
    fn adaptive_hysteresis(config: &JitterConfig) -> u32 {
        let cap = Self::comfort_cap_frames(config) as u32;
        (cap / 8).clamp(1, HYSTERESIS_BAND)
    }

    /// Adaptive dwell time: how many callbacks the raw target must stay
    /// outside the hysteresis band before committing to a new goal.
    /// High-cap presets (Auto, Resilient) react faster since they have
    /// more headroom. Low-cap presets stay conservative.
    ///
    /// **Upward transitions only.** Downward ones use [`DOWN_DWELL_MS`], which is
    /// an order of magnitude longer and deliberately not cap-scaled — see that
    /// constant for the measurement, and note that the reason this one may stay
    /// short is exactly the reason the other must not.
    fn adaptive_dwell(config: &JitterConfig) -> u32 {
        let cap = Self::comfort_cap_frames(config) as u32;
        40u32.saturating_sub((cap / 2).min(25)).max(15)
    }

    /// Dwell for a *downward* transition, in callbacks. Flat across presets: the
    /// arithmetic that makes a descent spurious (`0.1875 * max_gap` per decay step
    /// against a 3-frame dead-zone) depends on the link's measured `max_gap`, not
    /// on the preset's comfort cap, so scaling this by the cap would tune it on the
    /// wrong variable.
    fn down_dwell() -> u32 {
        DOWN_DWELL_MS / MILLIS_PER_FRAME
    }

    /// Pure computation of the target buffer depth, retaining every input term and
    /// which one won. The decision path and the log read the same struct, so an
    /// observability line can never drift from the behaviour it reports.
    ///
    /// Callers that only want the number take `.raw`.
    pub fn target_breakdown(
        &self,
        config: &JitterConfig,
        stats: &JitterStats,
        tcp_cap_override: Option<f32>,
    ) -> TargetBreakdown {
        // Static mode: lock buffer to exact user-specified depth, bypass all adaptive math.
        // min_depth is intentionally not applied — the user's explicit target is authoritative.
        if let Some(static_ms) = config.static_target_ms {
            let raw = ms_to_frames_ceil(static_ms);
            return TargetBreakdown {
                histogram_base: 0.0,
                burst_floor: 0.0,
                gap_floor: 0.0,
                min_depth: Self::min_depth_frames(config) as f32,
                pre_clamp: raw as f32,
                winning: TargetTerm::Static,
                raw,
            };
        }
        // NetEQ's base target: the 95th percentile of the relative arrival delay
        // histogram, fed NetEQ-style (running sum of per-packet IAT excess over a
        // 100-packet history), so a DTIM gap shows up in every packet of the burst
        // that follows it rather than as a single outlier.
        let histogram_base = stats.iat_percentile_target;
        // Reported only — deliberately NOT in the `.max()` chain below. See the
        // invariant comment there for the field measurement that removed it.
        let burst_floor = if stats.burst_detected() {
            stats.inter_burst_gap_frames() + 2.0
        } else {
            0.0
        };
        // Worst delivery gap in the recent window, plus headroom.
        // This is the primary depth signal on DTIM-batched links, where
        // per-packet jitter is ~0.05 frames but the inter-cluster gap is 20-60
        // frames — precisely the blind spot of a p95-of-packets statistic like
        // `histogram_base`. Unlike every other term here it needs no starvation
        // to learn the gap, and it ages out on its own.
        let gap = stats.max_gap_frames();
        let gap_floor = if gap > 1.0 {
            if stats.burst_detected() {
                // +25% while DTIM batching is detected. Doze gaps *grow* as the
                // radio sleeps deeper (the field log walks 21 → 48 → 106 frames),
                // and a purely reactive floor loses to a growing gap by exactly
                // one starvation every time. The headroom absorbs the next step
                // of the growth instead of stuttering on it.
                gap * 1.25 + 1.0
            } else {
                gap + 1.0
            }
        } else {
            0.0
        };
        // Every term here either ages out (gap window) or is recomputed from a
        // bounded history (histogram). Nothing latches its own peak — two removed
        // terms did (`peak_mode_height`, `cumsum_target`) and they held the target
        // at the comfort cap while this gap signal read 10 frames. Do not add a
        // term that cannot fall on its own.
        //
        // `burst_floor` (`inter_burst_gap + 2`) was the third such term. It did not
        // latch a *peak*, so it passed the letter of the rule above, but it was the
        // only **boolean-gated** term in the chain: it could not rise or fall, only
        // step, by whatever the un-aged EWMA happened to hold. Measured in the
        // field directly:
        //   - ADB (a *cable*): `inter_burst_gap` averaged 1485 frames, peaked at
        //     5783.5 (57.8s), while the honest `max_gap` on the same log averaged
        //     7.8 and peaked at 24. It won 168 of 472 depth decisions there.
        //   - 2.4GHz: `gap_floor` sat stable at ~39 while `burst_floor` swung
        //     43 → 313, producing 15 target jumps ≥20 frames. Removing it drops the
        //     average target 54.8 → 28.4 and the ≥20-frame jumps to 0.
        //   - Screen *off* — when DTIM batching should be at its worst — is when the
        //     term went quiet (10 of 126 lines burst=true, avg target 27.6). It was
        //     measuring scheduler coalescing, not radio behaviour.
        // The DTIM gap it was supposed to cover is already covered: on 2.4GHz with
        // burst=true, `max_gap` peaked at 39.0 frames while `gap_floor` peaked at
        // 49.8, via the burst-aware +25% headroom below. Upstream NetEQ has no
        // burst/cluster concept at all — one continuously-decaying histogram
        // (`histogram.cc:41-55`) is why its target cannot step.
        let min_depth = Self::min_depth_frames(config) as f32;
        let target = min_depth.max(histogram_base).max(gap_floor);
        // Ties resolve to the earlier term in the chain, so `min_depth` wins
        // whenever nothing *observed* strictly exceeds the floor. Attributing a
        // floor-height depth to the histogram would read as "the link demanded
        // this", which is the exact misreading the field diagnoses kept making.
        let winning = if min_depth >= histogram_base.max(gap_floor) {
            TargetTerm::MinDepth
        } else if histogram_base >= gap_floor {
            TargetTerm::Histogram
        } else {
            TargetTerm::GapFloor
        };
        let cap = tcp_cap_override.unwrap_or(Self::comfort_cap_frames(config));
        let safe_cap = cap.max(min_depth);
        let raw = target.ceil().clamp(min_depth, safe_cap) as u32;
        TargetBreakdown {
            histogram_base,
            burst_floor,
            gap_floor,
            min_depth,
            pre_clamp: target,
            winning,
            raw,
        }
    }

    /// NetEQ `DelayManager::BufferLimits` — the drain decision band around the
    /// effective target, in whole frames. The orchestrator compares the filtered
    /// buffer level against these:
    ///   - `filtered >= emergency` → emergency drain (fast accelerate, no cooldown),
    ///     where `emergency = high + max(50ms, high/2)` — see
    ///     `manager::emergency_threshold`
    ///   - `filtered >= high`      → gentle drain (normal accelerate, cooldown)
    ///   - `filtered <  low`       → preemptive expand (slow down)
    ///
    /// `low = 0.75 * target`; `high = max(target, low + MIN_BAND)`. The minimum
    /// spread guarantees a dead-band between the expand and accelerate decisions so
    /// we don't ping-pong between them. Mirrors `delay_manager.cc:358-375`
    /// (adapted from Q8 packets to whole frames).
    ///
    /// `MIN_BAND` only binds while `low + MIN_BAND > target`, i.e. below a target
    /// of `4 * MIN_BAND` — **8 frames** at the 20ms it is set to now, and 16 at
    /// the 40ms it used to be. Above that the band is already `target/4` and the
    /// constant is inert, which is what made the old value a mistake worth
    /// measuring rather than arguing about.
    pub fn buffer_limits(target: u32) -> Band {
        /// Minimum low→high spread in frames, matching NetEQ's `window_20ms`
        /// (`delay_manager.cc:417-426`).
        ///
        /// Was 40ms, widened from NetEQ's 20ms on the theory that 2.4GHz
        /// micro-oscillation would otherwise fire a splice per swing. The field
        /// shows the widening never reached that link: **2.4GHz runs a 32-frame
        /// target**, where `MIN_BAND` does not bind at all and the band is
        /// `target/4 = 8` frames either way. It bound on 100% of ADB windows and
        /// 60% of 5GHz ones — the two links where the cost is pure latency.
        ///
        /// On ADB the cost was the whole complaint. Target measured 5.3 frames,
        /// so `low = 3` and `high = max(5, 3+4) = 7`; measured `avg_filtered` was
        /// **7.4** against an `avg_high_limit` of **7.4**. The filtered level was
        /// parked exactly on its own trigger — arming in 45% of windows, draining
        /// a little, falling back inside the dead-band, and stalling at ~100ms on
        /// a link whose worst observed gap over the entire capture was 4.9 frames.
        /// At 20ms the same target gives `high = max(5, 3+2) = 5`, so the drain
        /// arms at the target itself; 2.4GHz is bit-identical.
        ///
        /// The 2.4GHz risk is not gone, only relocated: if a later change pulls
        /// that target under 8 frames, this constant starts binding there too and
        /// the per-oscillation stutter becomes possible again. The splice counters
        /// on the depth line make that visible directly.
        const MIN_BAND: u32 = 20 / super::consts::MILLIS_PER_FRAME;
        let low = target * 3 / 4;
        let high = target.max(low + MIN_BAND);
        Band { low, high }
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
        self.starvation_timestamps.clear();
        self.last_starvation = None;
        self.last_floor_decay = Instant::now();
        self.effective_target
    }

    /// Full reset on stream restart.
    /// Preserves `probe_floor` and `starvation_timestamps` — the network hasn't
    /// changed, only the stream died. Starting at the learned floor avoids the
    /// cold-start cascade (2→12→32→44→52 in <1s) measured on 2.4GHz.
    pub fn reset(&mut self) {
        self.effective_target = self.probe_floor.max(2);
        self.ramp_goal = self.effective_target;
        self.target_exit_count = 0;
        self.ramp_countdown = 0;
        self.probe_down_countdown = PROBE_DOWN_INTERVAL;
        // probe_floor: PRESERVED — empirical knowledge of starvation threshold.
        // starvation_timestamps: PRESERVED — gates probe aggression after restart.
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
        now: Instant,
    ) -> u32 {
        // Static and No Buffer modes bypass hysteresis entirely.
        if config.static_target_ms.is_some() {
            self.effective_target = raw_target;
            self.ramp_goal = raw_target;
            return raw_target;
        }

        let quantum = Self::adaptive_quantum(config);
        let hysteresis = Self::adaptive_hysteresis(config);
        let cap = Self::comfort_cap_frames(config) as u32;
        // Clamp AFTER quantization. `quantize_target` rounds to the nearest
        // multiple, which can land above the cap — `quantize_target(50, 4) == 52`
        // is why the 2.4GHz field log shows `effective_target=52, cap=50`.
        let quantized =
            Self::quantize_target(raw_target, quantum).clamp(min_depth, cap.max(min_depth));
        let diff = self.effective_target.abs_diff(quantized);

        // Relax the learned floor on a timer, independent of ramp direction.
        self.relax_probe_floor(quantum, min_depth, now);

        if diff <= hysteresis {
            // Inside the dead-zone: no change, reset dwell counter.
            self.target_exit_count = 0;
        } else {
            // Direction of the *pending* transition. A reversal — or a restart
            // after the dead-zone reset above — begins the dwell again, so the two
            // directions never pool evidence.
            let falling = quantized < self.effective_target;
            if self.target_exit_count == 0 || falling != self.exit_falling {
                self.exit_falling = falling;
                self.target_exit_count = 0;
            }
            self.target_exit_count += 1;
            let dwell = if falling {
                Self::down_dwell()
            } else {
                Self::adaptive_dwell(config)
            };
            if self.target_exit_count >= dwell {
                // Sustained deviation — commit to new ramp goal.
                tracing::debug!(
                    "[JitterMgr] Target transition ({}): effective={}→ramp_goal={}, raw={}, dwell={}, ema_jitter={:.2}, max_gap={:.1}, floor={}, stability={:.2}",
                    if falling { "down" } else { "up" },
                    self.effective_target,
                    quantized,
                    raw_target,
                    dwell,
                    stats.ema_jitter,
                    stats.max_gap_frames(),
                    self.probe_floor,
                    stats.stability_ratio(),
                );
                self.ramp_goal = quantized.max(self.probe_floor);
                self.target_exit_count = 0;
            }
        }

        // Rate-limited ramp toward the goal, in BOTH directions.
        // UP: RAMP_UP_STEP frames per RAMP_INTERVAL_UP callbacks. Fast enough to
        //     beat real degradation, gradual enough that the decision band's
        //     `low_limit` stays within a frame or two of the real buffer level —
        //     the instant jump this replaces was the source of the click train.
        // DOWN: 1 frame per RAMP_INTERVAL — we have excess buffer, no urgency,
        //       and slow descent lets the network prove stability.
        if self.effective_target != self.ramp_goal {
            if self.effective_target < self.ramp_goal {
                if self.ramp_countdown == 0 {
                    self.effective_target = self
                        .effective_target
                        .saturating_add(RAMP_UP_STEP)
                        .min(self.ramp_goal);
                    self.ramp_countdown = RAMP_INTERVAL_UP;
                }
            } else if self.ramp_countdown == 0 {
                self.effective_target -= 1;
                // Never ramp below the learned starvation floor — that level
                // caused starvation before and will again.
                if self.effective_target < self.probe_floor {
                    tracing::debug!(
                        "[JitterMgr] probe_floor holding: effective clamped {}→{}, ramp_goal={}, floor={}",
                        self.effective_target,
                        self.probe_floor,
                        self.ramp_goal,
                        self.probe_floor,
                    );
                    self.effective_target = self.probe_floor;
                }
                self.ramp_countdown = RAMP_INTERVAL;
            }
            self.ramp_countdown = self.ramp_countdown.saturating_sub(1);
        } else if stats.stability_ratio() > 0.2
            && self.effective_target > min_depth
            // Allow probing even during unstable regime if current stability
            // is locally high enough — the regime lock is a coarse heuristic
            // and the probe_floor prevents re-probing below safe levels.
            && !stats
                .unstable_regime_until()
                .is_some_and(|until| now < until && stats.stability_ratio() < 0.5)
        {
            // Active downward probing: when the network has been calm for
            // a sustained period, nudge the target down to discover the
            // lowest stable depth. Speed scales with confidence.
            //
            // Burst clustering no longer *blocks* probing, it only slows it to
            // the gated interval. A blanket block meant the target could only
            // ever move up for as long as the screen was off — and since
            // `max_gap_frames()` now guards the depth honestly, the buffer no
            // longer needs a probe veto to stay safe through a DTIM cycle.
            let probe_interval = if self.recent_starvation_count(now) > 0 || stats.burst_detected()
            {
                PROBE_GATED_INTERVAL
            } else if stats.stability_ratio() > 0.8 {
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
    /// probing won't immediately descend back to the level that just starved.
    ///
    /// Deliberately does NOT consult `compute_target_depth` any more. That ran
    /// ~140ms into recovery, when `iat_percentile_target` and
    /// `inter_burst_gap_frames` are both inflated **by the starvation itself** —
    /// a positive feedback loop that took the floor 5→20 and 23→40 in single
    /// events. The honest depth signal is `stats.max_gap_frames()`, read live
    /// through `compute_target_depth` on the normal path where it can also fall
    /// again.
    ///
    /// What is left is a pure safety net for starvations no arrival gap explains
    /// (decode stall, CPU scheduling): one quantum above whatever depth starved,
    /// ceilinged at `cap/2` so the floor ALONE can never pin the target at the
    /// comfort cap.
    ///
    /// **The gate below is that sentence made executable.** This once raised the
    /// floor unconditionally, including for the starvations the arrival gap
    /// explains perfectly well. Measured at the logged "Starvation floor set"
    /// events across four captures, `max_gap >= effective_target` held in
    /// **53 / 65 / 43 / 58%** of them — the majority of ratchets were cases the
    /// honest signal had already priced.
    ///
    /// Those cases are redundant *by construction*, not by measurement. When the
    /// gap term is live (`gap > 1.0`) `compute_target_depth` returns
    /// `gap_floor >= gap + 1.0` — the burst branch adds a further 25%, so the
    /// bound holds either way — hence `gap >= effective_target` implies
    /// `gap_floor >= effective_target + 1`, and `raw_target` clears the current
    /// target on the very next window with no floor involved. The `gap > 1.0`
    /// conjunct is load-bearing and not cosmetic: below it `gap_floor` is
    /// hard-zeroed, so a bare `gap >= effective_target` would decline the ratchet
    /// at `effective_target <= 1` while nothing at all replaced it.
    ///
    /// Worth ~6ms of mean latency, which is not why it is here. An unconditional
    /// ratchet is one bad interaction away from becoming another statistic that
    /// latches its own history, against a module whose central invariant is that
    /// none may. It is *not* one today — floor falls outnumber rises 136:34 on
    /// 24-unc and 90:24 on 24-128k — and the point of the gate is to keep that
    /// true by construction rather than by luck.
    pub fn apply_starvation_floor(&mut self, config: &JitterConfig, stats: &JitterStats) {
        let quantum = Self::adaptive_quantum(config);
        let cap = Self::comfort_cap_frames(config) as u32;
        let gap = stats.max_gap_frames();
        if gap > 1.0 && gap >= self.effective_target as f32 {
            tracing::info!(
                "[JitterMgr] Starvation floor declined (gap already covers it): probe_floor={}, effective_target={}, cap={}, max_gap={:.1}, ema_jitter={:.2}",
                self.probe_floor,
                self.effective_target,
                cap,
                gap,
                stats.ema_jitter,
            );
            return;
        }
        self.probe_floor = self
            .probe_floor
            .max(self.effective_target.saturating_add(quantum))
            .min((cap / 2).max(1));
        tracing::info!(
            "[JitterMgr] Starvation floor set: probe_floor={}, effective_target={}, cap={}, max_gap={:.1}, ema_jitter={:.2}",
            self.probe_floor,
            self.effective_target,
            cap,
            gap,
            stats.ema_jitter,
        );
    }

    /// Relax the learned floor by one quantum per [`FLOOR_DECAY_SECS`], once
    /// [`FLOOR_HOLD_SECS`] of starvation-free playback have elapsed.
    ///
    /// This is the descent guarantee. The previous decay path lived inside the
    /// dwell-completion branch and required the quantized target to be moving UP
    /// *and* zero recent starvation — but when the floor is what's holding the
    /// target high, the quantized target is *below* `effective_target`, so the
    /// branch was unreachable. The floor became a one-way ratchet, and once it
    /// reached the comfort cap the target was pinned there permanently.
    fn relax_probe_floor(&mut self, quantum: u32, min_depth: u32, now: Instant) {
        if self.probe_floor <= min_depth {
            return;
        }
        if self
            .last_starvation
            .is_some_and(|t| now.duration_since(t).as_secs() < FLOOR_HOLD_SECS)
        {
            return;
        }
        if now.duration_since(self.last_floor_decay).as_secs() < FLOOR_DECAY_SECS {
            return;
        }
        self.last_floor_decay = now;
        let relaxed = self.probe_floor.saturating_sub(quantum).max(min_depth);
        tracing::debug!(
            "[JitterMgr] probe_floor relaxing: {}→{}",
            self.probe_floor,
            relaxed,
        );
        self.probe_floor = relaxed;
    }

    /// Immediately jump `effective_target` and `ramp_goal` to `probe_floor` after
    /// starvation recovery. Eliminates the window where the buffer is undersized
    /// while the ramp catches up — every callback in that window is a starvation
    /// risk. Safe to do instantly (unlike a normal ramp-up) because the buffer is
    /// already empty here: there is no audio in flight for a splice to click on.
    pub fn jump_to_floor(&mut self) {
        if self.probe_floor > self.effective_target {
            tracing::debug!(
                "[JitterMgr] Starvation recovery jump: effective={}→{}",
                self.effective_target,
                self.probe_floor,
            );
            self.effective_target = self.probe_floor;
            self.ramp_goal = self.probe_floor;
            self.target_exit_count = 0;
        }
    }

    /// Record a starvation event for rate-limiting probe aggressiveness.
    /// Evicts timestamps older than STARVATION_WINDOW_SECS.
    pub fn record_starvation(&mut self, now: Instant) {
        self.starvation_timestamps.push_back(now);
        self.last_starvation = Some(now);
        // Evict expired entries. Compare forward with `duration_since` rather than
        // `now - WINDOW`: the subtraction underflows (and panics on Windows) when
        // `now` is within the window of the `Instant` epoch, which a test's
        // constructed base can be. Stored stamps are always <= `now`.
        let window = std::time::Duration::from_secs(STARVATION_WINDOW_SECS);
        while self
            .starvation_timestamps
            .front()
            .is_some_and(|&t| now.duration_since(t) > window)
        {
            self.starvation_timestamps.pop_front();
        }
    }

    /// How many starvation events occurred within the tracking window.
    fn recent_starvation_count(&self, now: Instant) -> usize {
        // Forward comparison, not `now - WINDOW`: see `record_starvation`.
        let window = std::time::Duration::from_secs(STARVATION_WINDOW_SECS);
        self.starvation_timestamps
            .iter()
            .filter(|&&t| now.duration_since(t) <= window)
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// 2.4GHz-shaped profile: 30ms floor, 800ms cap.
    fn cfg() -> JitterConfig {
        JitterConfig {
            min_depth_ms: 30,
            comfort_cap_ms: 800,
            peak_decay_halflife_ms: 0,
            resume_threshold_pct: 0.5,
            static_target_ms: None,
        }
    }

    /// Regression for a dominant field bug: `probe_floor` was a one-way ratchet
    /// that walked to the comfort cap and pinned `effective_target` there forever.
    /// Two independent guarantees now prevent that — a hard `cap/2` ceiling and a
    /// starvation-free decay timer — and this asserts both.
    #[test]
    fn probe_floor_is_ceilinged_at_half_cap_and_decays_to_min_depth() {
        let config = cfg();
        let stats = JitterStats::new();
        let cap = TargetController::comfort_cap_frames(&config) as u32; // 80
        let min_depth = TargetController::min_depth_frames(&config); // 3
        let mut control = TargetController::new();

        // Hammer the floor from a target already at the cap — the exact shape of
        // the field cascade. It must saturate at cap/2, never at cap.
        control.effective_target = cap;
        for _ in 0..50 {
            control.apply_starvation_floor(&config, &stats);
        }
        assert_eq!(
            control.probe_floor,
            cap / 2,
            "the learned floor alone must never be able to pin the target at the comfort cap",
        );

        // Now grant sustained starvation-free time and let the ramp run. The
        // floor must walk all the way back down to min_depth on its own.
        // The stored stamps stay at a real base; elapsed time is simulated by
        // advancing the `now` we pass forward. Only ever *add* to an `Instant` —
        // subtracting from `Instant::now()` underflows and panics on Windows when
        // uptime is below the subtracted duration.
        let base = Instant::now();
        control.last_starvation = Some(base);
        control.last_floor_decay = base;
        let now = base + Duration::from_secs(3600);
        for _ in 0..200 {
            control.advance(&config, &stats, min_depth, min_depth, now);
            // Each relaxation is gated on FLOOR_DECAY_SECS of wall clock, which a
            // unit test can't wait out — hold the decay stamp in the past relative
            // to `now` so we exercise the decay path rather than the timer.
            control.last_floor_decay = base;
        }
        assert_eq!(
            control.probe_floor, min_depth,
            "a starvation-free link must be able to relax the floor all the way down",
        );
    }

    /// A fresh starvation must re-arm the hold: the floor may not decay while
    /// the link is still misbehaving.
    #[test]
    fn probe_floor_holds_while_starvations_are_recent() {
        let config = cfg();
        let stats = JitterStats::new();
        let min_depth = TargetController::min_depth_frames(&config);
        let mut control = TargetController::new();
        control.effective_target = 20;
        let base = Instant::now();
        control.record_starvation(base);
        control.apply_starvation_floor(&config, &stats);
        let floor_after_starvation = control.probe_floor;
        assert!(floor_after_starvation > min_depth);

        // `last_floor_decay` is stale, but the starvation is fresh — the hold wins.
        // `now` is only 1s past the starvation, inside FLOOR_HOLD_SECS, while the
        // decay stamp sits far enough back to have relaxed had the hold not won.
        let now = base + Duration::from_secs(1);
        control.last_floor_decay = base;
        for _ in 0..100 {
            control.advance(&config, &stats, min_depth, min_depth, now);
        }
        assert_eq!(
            control.probe_floor, floor_after_starvation,
            "floor must not relax within FLOOR_HOLD_SECS of a starvation",
        );
    }

    /// The floor's own doc calls it "a pure safety net for starvations no arrival
    /// gap explains", and the gate makes that sentence executable. Both halves are
    /// pinned here, because a gate that never declines and a gate that always
    /// declines are equally easy to ship and equally invisible in a green suite.
    mod the_starvation_floor_is_a_net_only_for_starvations_no_gap_explains {
        use super::*;

        /// The 53-65% case in the field captures: the buffer starved at a depth the
        /// delivery-gap window had already measured past. Ratcheting there buys
        /// nothing the honest signal was not about to deliver on its own — so this
        /// asserts *both* halves. Without the second assertion the test would pass
        /// just as happily against a gate that suppressed a floor nothing replaced.
        #[test]
        fn a_starvation_the_gap_signal_already_explains_should_not_ratchet_the_floor() {
            let config = cfg();
            let mut control = TargetController::new();
            control.effective_target = 20;

            // A 240ms delivery gap — deeper than the depth that just starved.
            let mut stats = JitterStats::new();
            stats.record_gap(24.0, Instant::now());
            assert!(
                stats.max_gap_frames() >= control.effective_target as f32,
                "precondition: the gap window must already cover the starved depth",
            );

            let unaided = control.target_breakdown(&config, &stats, None);
            assert!(
                unaided.raw > control.effective_target,
                "the gap term alone must already lift the target above the starved depth ({} vs {})",
                unaided.raw,
                control.effective_target,
            );

            control.apply_starvation_floor(&config, &stats);
            assert_eq!(
                control.probe_floor, 0,
                "a starvation the gap window explains must leave the learned floor alone",
            );
        }

        /// The complementary half — three shapes the gate must let through:
        ///   1. nothing in the gap window at all: the decode-stall / scheduling
        ///      case the net exists for;
        ///   2. a real gap, but shallower than the depth that starved, so the gap
        ///      cannot be the whole story;
        ///   3. a gap too small for `gap_floor` to be live at all, which is why the
        ///      guard reads `gap > 1.0 && gap >= target` and not just the
        ///      comparison — a bare comparison declines here while
        ///      `compute_target_depth` hard-zeroes the term that was supposed to
        ///      take over.
        #[test]
        fn a_starvation_with_no_arrival_gap_should_still_ratchet_the_floor() {
            let config = cfg();
            let quantum = TargetController::adaptive_quantum(&config);

            let mut control = TargetController::new();
            control.effective_target = 20;
            let stats = JitterStats::new();
            assert!(
                stats.max_gap_frames() < 1.0,
                "precondition: no gap observed"
            );
            control.apply_starvation_floor(&config, &stats);
            assert_eq!(
                control.probe_floor,
                20 + quantum,
                "a starvation no gap explains is exactly what the floor is for",
            );

            let mut control = TargetController::new();
            control.effective_target = 20;
            let mut stats = JitterStats::new();
            stats.record_gap(10.0, Instant::now());
            assert!(
                stats.max_gap_frames() < control.effective_target as f32,
                "precondition: the gap must be shallower than the depth that starved",
            );
            control.apply_starvation_floor(&config, &stats);
            assert_eq!(
                control.probe_floor,
                20 + quantum,
                "a gap too shallow to explain the starvation must not disarm the net",
            );

            let mut control = TargetController::new();
            control.effective_target = 1;
            let mut stats = JitterStats::new();
            stats.record_gap(1.0, Instant::now());
            assert!(
                stats.max_gap_frames() >= control.effective_target as f32,
                "precondition: the bare comparison holds here — only `gap > 1.0` saves it",
            );
            assert!(
                control.target_breakdown(&config, &stats, None).gap_floor < 1e-6,
                "precondition: at or below 1.0 frames the gap term is not live",
            );
            control.apply_starvation_floor(&config, &stats);
            assert_eq!(
                control.probe_floor,
                1 + quantum,
                "the floor may only stand down for a gap term that is actually standing up",
            );
        }
    }

    /// The field log shows `effective_target=52, cap=50`: `quantize_target`
    /// rounds to the nearest multiple, which can land *above* the cap. The clamp
    /// now happens after quantization.
    #[test]
    fn effective_target_never_exceeds_the_comfort_cap() {
        // 500ms cap reproduces the exact logged case.
        let config = JitterConfig {
            comfort_cap_ms: 500,
            ..cfg()
        };
        let stats = JitterStats::new();
        let cap = TargetController::comfort_cap_frames(&config) as u32; // 50
        let min_depth = TargetController::min_depth_frames(&config);
        let mut control = TargetController::new();

        let now = Instant::now();
        for _ in 0..5000 {
            // Raw target far above the cap, sustained — the worst case.
            let target = control.advance(&config, &stats, cap * 4, min_depth, now);
            assert!(
                target <= cap,
                "effective_target {target} exceeded comfort cap {cap}",
            );
        }
        assert!(
            control.effective_target >= cap - 4,
            "sustained over-cap demand should still pin the target near the cap, got {}",
            control.effective_target,
        );
    }

    /// Click-train fix #1. An instant 15-frame jump in `effective_target` drops
    /// `low_limit` far below the real buffer level, and every callback in that
    /// window used to fire a preemptive WSOLA expand — the "fast clicking on
    /// every buffer increase" from the field test. Growth is now rate-limited.
    #[test]
    fn upward_ramp_is_rate_limited() {
        let config = cfg();
        let stats = JitterStats::new();
        let min_depth = TargetController::min_depth_frames(&config);
        let mut control = TargetController::new();

        let mut prev = control.effective_target;
        let mut worst_step = 0;
        let now = Instant::now();
        for _ in 0..4000 {
            let target = control.advance(&config, &stats, 60, min_depth, now);
            worst_step = worst_step.max(target.saturating_sub(prev));
            prev = target;
        }
        assert!(
            worst_step <= RAMP_UP_STEP,
            "single-callback target increase of {worst_step} frames exceeds RAMP_UP_STEP={RAMP_UP_STEP}",
        );
        assert!(
            control.effective_target >= 40,
            "the rate limit must not stop the climb, only pace it — got {}",
            control.effective_target,
        );
    }

    /// The gap tracker must be able to raise the target on its own, without a
    /// starvation to teach it — that is the whole point of the signal.
    #[test]
    fn observed_delivery_gap_raises_the_target_without_starving() {
        let config = cfg();
        let mut stats = JitterStats::new();
        let control = TargetController::new();
        let baseline = control.target_breakdown(&config, &stats, None).raw;

        // One 200ms DTIM-shaped gap.
        let t0 = Instant::now();
        stats.observe(1, t0);
        stats.observe(2, t0 + Duration::from_millis(210));

        let raised = control.target_breakdown(&config, &stats, None).raw;
        assert!(
            raised >= 21 && raised > baseline,
            "a 200ms delivery gap must lift the target to at least 21 frames (was {baseline}, got {raised})",
        );
    }

    /// `inter_burst_gap` was once the fourth term of the `.max()` chain and the
    /// only boolean-gated one, so it could not rise or fall — only step. The field
    /// measured it at 5783.5 frames (57.8s) on a USB cable while `max_gap` on the
    /// same capture peaked at 24, and it won 168 of 472 ADB depth decisions. These
    /// tests fix the division of labour it was removed for: burst detection may
    /// buy *headroom* on a gap the link actually delivered, and may not invent
    /// depth on its own.
    ///
    /// The disagreement is injected with `force_burst_state` rather than grown from
    /// arrivals on purpose. Driving the real detector makes the two signals *agree* —
    /// a synthetic inter-cluster silence is also a real delivery gap, so the fixture
    /// would prove nothing. In the field they diverge either through the stale anchor
    /// (a `stats` defect, tested there) or through their different ageing rules. What
    /// belongs here is what the depth authority does with a divergence it did not
    /// cause.
    mod burst_detection_must_not_own_the_depth {
        use super::*;

        /// The ADB pathology, reduced: burst active, an absurd inter-burst gap, and
        /// a link that delivered everything on time. The 4-frame histogram seed
        /// (`reset_histogram_to_seed`) is the honest cold-start depth and is what
        /// must win — not a 57.8s phantom.
        #[test]
        fn a_stale_inter_burst_gap_must_not_reach_the_depth_authority() {
            let config = cfg();
            let control = TargetController::new();
            let mut stats = JitterStats::new();
            // The measured ADB peak, verbatim.
            stats.force_burst_state(true, 5783.5);

            let b = control.target_breakdown(&config, &stats, None);
            assert!(
                b.burst_floor > 5000.0,
                "the term must still be computed for the log, got {:.1}",
                b.burst_floor,
            );
            assert_eq!(
                b.winning,
                TargetTerm::Histogram,
                "nothing was delivered late, so only the cold-start seed may set the \
                 depth: raw={}, burst_floor={:.1}, pre_clamp={:.1}",
                b.raw,
                b.burst_floor,
                b.pre_clamp,
            );
            assert!(
                b.raw <= 4,
                "a 57.8s phantom gap must cost zero latency, got raw={}",
                b.raw,
            );
        }

        /// The general form: whatever the burst term holds, the depth must be
        /// exactly what the *honest* terms justify. Sweeps the delivered gap across
        /// the range the field logs produced (ADB 7.8 avg / 24 peak, 2.4GHz 25.6 avg
        /// / 39 peak) against a burst term an order of magnitude above each.
        #[test]
        fn burst_detection_should_not_raise_the_target_above_the_measured_gap() {
            let config = cfg();
            let control = TargetController::new();

            for gap in [0.0, 7.8, 24.0, 25.6, 39.0] {
                let mut stats = JitterStats::new();
                if gap > 0.0 {
                    stats.record_gap(gap, Instant::now());
                }
                stats.force_burst_state(true, gap.max(1.0) * 10.0 + 300.0);

                let b = control.target_breakdown(&config, &stats, None);
                // What the honest signals justify, with `burst_floor` excluded by
                // construction: the config floor, the histogram, or the burst-aware
                // gap floor — whichever is largest.
                let honest = b.min_depth.max(b.histogram_base).max(b.gap_floor);
                assert!(
                    (b.pre_clamp - honest).abs() < 1e-3,
                    "gap={gap}: depth {:.1} != the honest maximum {honest:.1} — the burst \
                     term is back in the .max() chain (burst_floor={:.1})",
                    b.pre_clamp,
                    b.burst_floor,
                );
            }
        }

        /// The half of the mechanism that stays. `burst_detected` still widens
        /// `gap_floor` by 25%, and that headroom is what actually covered the
        /// measured 2.4GHz DTIM gap: `max_gap` peaked at 39.0 frames while
        /// `gap_floor` peaked at 49.8, with room to spare.
        #[test]
        fn burst_detection_should_still_widen_the_gap_floor_headroom() {
            let config = cfg();
            let control = TargetController::new();
            let gap = 39.0; // the measured 2.4GHz peak

            let mut quiet = JitterStats::new();
            quiet.record_gap(gap, Instant::now());
            assert!(!quiet.burst_detected());
            let without = control.target_breakdown(&config, &quiet, None);

            let mut bursty = JitterStats::new();
            bursty.record_gap(gap, Instant::now());
            // A burst term *below* the gap floor, so the only thing under test is
            // the headroom multiplier and not the removed term leaking back in.
            bursty.force_burst_state(true, 1.0);
            let with = control.target_breakdown(&config, &bursty, None);

            assert!(
                with.gap_floor > without.gap_floor,
                "burst detection must still buy headroom on the delivered gap: \
                 {:.1} (burst) vs {:.1} (quiet)",
                with.gap_floor,
                without.gap_floor,
            );
            assert!(
                (with.gap_floor - (gap * 1.25 + 1.0)).abs() < 1e-3,
                "the burst-aware gap floor must be gap*1.25+1, got {:.1}",
                with.gap_floor,
            );
            assert!(
                with.gap_floor >= gap,
                "the headroom must still cover the gap it is sized for: {:.1} < {gap}",
                with.gap_floor,
            );
        }

        /// The real detector, end to end: a genuine DTIM-shaped batching pattern
        /// must still produce a depth that covers it. Removing the term must not
        /// leave batched links uncovered — the property `burst_floor` was once
        /// wrongly believed to be the only source of.
        #[test]
        fn a_real_batching_pattern_must_still_be_covered_without_the_burst_term() {
            let config = cfg();
            let control = TargetController::new();
            let mut stats = JitterStats::new();

            // Three tight 4-packet clusters, 200ms apart — a DTIM cycle.
            let mut at = Instant::now();
            let mut seq = 1u64;
            for round in 0..3 {
                for _ in 0..4 {
                    stats.observe(seq, at);
                    seq += 1;
                    at += Duration::from_millis(1);
                }
                if round < 2 {
                    at += Duration::from_millis(200);
                }
            }

            assert!(
                stats.burst_detected(),
                "fixture must trip the real detector",
            );
            let b = control.target_breakdown(&config, &stats, None);
            let honest = b.min_depth.max(b.histogram_base).max(b.gap_floor);
            assert!(
                (b.pre_clamp - honest).abs() < 1e-3,
                "a real batching pattern must still be sized by the honest terms: \
                 pre_clamp={:.1} vs honest={honest:.1} (burst_floor={:.1})",
                b.pre_clamp,
                b.burst_floor,
            );
            assert!(
                b.raw as f32 >= stats.max_gap_frames(),
                "the target must still cover the delivered gap: raw={} vs max_gap={:.1}",
                b.raw,
                stats.max_gap_frames(),
            );
            assert!(
                b.gap_floor >= stats.max_gap_frames(),
                "the honest gap signal must be the one carrying a batched link now: \
                 gap_floor={:.1} vs max_gap={:.1}",
                b.gap_floor,
                stats.max_gap_frames(),
            );
        }
    }

    /// The gap window in **isolation**, which is what makes it load-bearing.
    /// In the field a DTIM gap affects one arrival in 20-60, so it sits
    /// above the 95th percentile *of packets* — `iat_percentile_target` cannot see
    /// it, and `burst_detected` needs a full cluster pattern to fire. Populating
    /// only the gap window reproduces that blind spot exactly: every other term in
    /// the `.max()` chain reads zero, and the target must still cover the gap.
    #[test]
    fn the_gap_window_alone_is_enough_to_cover_a_dtim_gap() {
        let config = cfg();
        let mut stats = JitterStats::new();
        let control = TargetController::new();
        let min_depth = TargetController::min_depth_frames(&config);
        let baseline = control.target_breakdown(&config, &stats, None).raw;
        assert!(
            baseline <= min_depth + 2,
            "a virgin stats object must sit near the floor (min_depth={min_depth}), got {baseline}",
        );

        // A 500ms gap, recorded with nothing else touched.
        stats.record_gap(50.0, Instant::now());
        let raised = control.target_breakdown(&config, &stats, None).raw;
        assert!(
            raised >= 51,
            "the delivery-gap window alone must cover a 500ms gap (≥51 frames), got {raised}",
        );
        assert!(
            raised <= TargetController::comfort_cap_frames(&config) as u32,
            "still bounded by the comfort cap",
        );
    }

    /// `winning_term` is the field's only cross-check on which statistic is driving
    /// the depth, so a wrong attribution is worse than no log at all — it is what
    /// three rounds of diagnosis got wrong by hand. Two properties are pinned:
    /// the reported term must be the one that actually produced `pre_clamp`, and a
    /// depth that is merely the config floor must never be attributed to an
    /// observation.
    #[test]
    fn the_reported_winning_term_must_be_the_one_that_produced_the_target() {
        let config = cfg();
        let control = TargetController::new();

        // Virgin stats: the cold-start histogram seed is 4 frames (NetEQ's
        // `ResetHistogram` geometric distribution puts p95 at bin 4), which is
        // *above* this config's 3-frame floor. So a fresh target of 4 is the
        // seed's claim, not the floor's, and the log must say so — otherwise a
        // cold start reads identically to an observed 4-frame demand.
        let stats = JitterStats::new();
        let b = control.target_breakdown(&config, &stats, None);
        assert_eq!(
            b.winning,
            TargetTerm::Histogram,
            "the cold-start seed outranks this config's floor (histogram={:.1}, \
             min_depth={:.1}) and must be reported as the source",
            b.histogram_base,
            b.min_depth,
        );

        // A genuine tie, where the floor and the seed are the same height: it
        // must resolve to `min_depth`, so a depth that is merely the configured
        // floor is never credited to an observation. This is the exact tie the
        // earlier draft of this attribution got backwards.
        let tied = JitterConfig {
            min_depth_ms: 40, // == the 4-frame histogram seed
            ..cfg()
        };
        let b = control.target_breakdown(&tied, &JitterStats::new(), None);
        assert_eq!(b.min_depth, b.histogram_base, "precondition: a real tie");
        assert_eq!(
            b.winning,
            TargetTerm::MinDepth,
            "a tie must fall to the floor, not to the statistic that only matches it",
        );

        // An isolated delivery gap: only `gap_floor` can see it.
        let mut stats = JitterStats::new();
        stats.record_gap(50.0, Instant::now());
        let b = control.target_breakdown(&config, &stats, None);
        assert_eq!(b.winning, TargetTerm::GapFloor);
        assert!(
            (b.pre_clamp - b.gap_floor).abs() < 1e-6,
            "pre_clamp ({:.1}) must equal the winning term gap_floor ({:.1})",
            b.pre_clamp,
            b.gap_floor,
        );

        // Whatever the inputs, the named term must always be the maximum of the
        // chain — the invariant that makes the log line trustworthy.
        for gap in [0.0, 3.4, 24.6, 45.6, 200.0] {
            let mut stats = JitterStats::new();
            if gap > 0.0 {
                stats.record_gap(gap, Instant::now());
            }
            let b = control.target_breakdown(&config, &stats, None);
            let reported = match b.winning {
                TargetTerm::MinDepth => b.min_depth,
                TargetTerm::Histogram => b.histogram_base,
                TargetTerm::GapFloor => b.gap_floor,
                TargetTerm::Static => unreachable!("adaptive config"),
            };
            assert!(
                (reported - b.pre_clamp).abs() < 1e-6,
                "gap={gap}: reported term {} = {reported:.1} but pre_clamp = {:.1}",
                b.winning.as_str(),
                b.pre_clamp,
            );
        }
    }

    /// A saturated statistic and a genuinely bad link produce the same `raw` once
    /// the comfort cap clamps, and only `pre_clamp` tells them apart. This is the
    /// distinction a 5GHz starvation storm needed and the log could not make: a histogram
    /// pinned at its 63-bin ceiling reads far above the cap, while an honest
    /// mid-range demand lands inside it.
    #[test]
    fn pre_clamp_must_reveal_a_demand_the_comfort_cap_hides() {
        let config = cfg();
        let control = TargetController::new();
        let cap = TargetController::comfort_cap_frames(&config);

        let mut stats = JitterStats::new();
        stats.record_gap(cap * 3.0, Instant::now());
        let b = control.target_breakdown(&config, &stats, None);
        assert_eq!(b.raw, cap as u32, "raw is clamped to the comfort cap");
        assert!(
            b.pre_clamp > cap,
            "pre_clamp ({:.1}) must expose the demand above the cap ({cap:.1}) that \
             raw ({}) cannot show",
            b.pre_clamp,
            b.raw,
        );
    }

    /// A field bug as a unit test. On 2.4GHz Router A the field recorded
    /// `effective_target=80` (the comfort cap) while `max_gap=9.7` — the only
    /// terms that could produce that were `max_iat_cumulative_sum` (a peak-latch
    /// of the running late-excess sum) and `peak_mode_height` (up to 8 latched
    /// peaks). Both re-measured the starvation-recovery burst that the gap window
    /// had already recorded once, then held that peak long after the window let
    /// go.
    ///
    /// Two guarantees, both asserted here:
    ///   1. **Bounded**: while the abuse is happening, the target may never
    ///      exceed what the honest gap signal justifies plus its headroom.
    ///   2. **Falls**: once the abuse stops, the target must collapse back to the
    ///      floor. This is the half a latch cannot satisfy — re-adding either one
    ///      leaves the target elevated for tens of seconds of clean link.
    #[test]
    fn no_statistic_may_latch_the_target_above_the_observed_gap() {
        let config = cfg();
        let mut stats = JitterStats::new();
        let control = TargetController::new();
        let min_depth = TargetController::min_depth_frames(&config);
        let quantum = TargetController::adaptive_quantum(&config);

        // Ten starve → recover → burst cycles: 361ms of silence, then the 40-frame
        // recovery burst delivered 1ms apart. Real-time honest (401ms of wall clock
        // for 41 frames of audio) so nothing drifts; the only real signal is a
        // repeating ~36-frame delivery gap.
        let mut t = Instant::now();
        let mut seq = 1u64;
        stats.observe(seq, t);
        for _ in 0..10 {
            t += Duration::from_millis(361);
            seq += 1;
            stats.observe(seq, t);
            for _ in 0..40 {
                t += Duration::from_millis(1);
                seq += 1;
                stats.observe(seq, t);
                let target = control.target_breakdown(&config, &stats, None).raw;
                let bound = (min_depth as f32)
                    .max(stats.max_gap_frames() * 1.25 + quantum as f32 + 1.0)
                    .ceil() as u32;
                assert!(
                    target <= bound,
                    "target {target} exceeds what max_gap={:.1} can justify (bound {bound}) — \
                     something in the .max() chain is latching its own history",
                    stats.max_gap_frames(),
                );
            }
        }

        // Abuse over. 22s of a perfect link must return the target to the floor.
        for _ in 0..2200 {
            t += Duration::from_millis(10);
            seq += 1;
            stats.observe(seq, t);
        }
        let settled = control.target_breakdown(&config, &stats, None).raw;
        assert!(
            settled <= min_depth + quantum,
            "after 22s of a clean link the target must be back at the floor \
             (min_depth={min_depth}), got {settled}",
        );
    }

    /// The drain dead-band, at the targets where the constant actually binds.
    ///
    /// NetEQ's `window_20ms` (`delay_manager.cc:417-426`) is 20ms; we ran 40ms
    /// for three rounds on the theory that it absorbed 2.4GHz micro-oscillation.
    /// It never reached that link — 2.4GHz measured a 32-frame target, far above
    /// where the constant binds — and on ADB it raised the drain trigger to 7
    /// frames against a measured target of 5.3, which is the ~100ms the buffer
    /// parked at.
    mod the_drain_dead_band_governs_only_small_targets {
        use super::*;

        #[test]
        fn the_drain_dead_band_should_be_twenty_milliseconds_at_a_small_target() {
            // ADB's measured target. `low = 3`; the band must be 20ms, not 40ms,
            // so `high` lands on the target instead of 2 frames above it.
            let Band { low, high } = TargetController::buffer_limits(5);
            assert_eq!((low, high), (3, 5), "40ms would have given (3, 7)");
            assert_eq!(
                high - low,
                20 / crate::jitter::consts::MILLIS_PER_FRAME,
                "the dead-band floor is NetEQ's 20ms",
            );

            // The smallest targets, where the floor is the only thing holding the
            // band open at all.
            assert_eq!(TargetController::buffer_limits(2), Band { low: 1, high: 3 });
            assert_eq!(TargetController::buffer_limits(4), Band { low: 3, high: 5 });
        }

        #[test]
        fn the_drain_dead_band_should_stay_proportional_at_a_large_target() {
            // 2.4GHz's measured target. The band is `target/4` here and the
            // constant is inert — this change is bit-identical on that link,
            // which is what makes it safe to make globally rather than per-link.
            let Band { low, high } = TargetController::buffer_limits(32);
            assert_eq!((low, high), (24, 32));
            assert_eq!(high - low, 8, "target/4, not the MIN_BAND floor");

            // 5GHz's measured target, just above where the floor lets go.
            assert_eq!(
                TargetController::buffer_limits(13),
                Band { low: 9, high: 13 }
            );
        }

        /// The ADB complaint in one assertion. `high` is the level the filtered
        /// buffer drains *down to*, so a `high` above `target` is latency the
        /// controller asked for and the drain then refuses to give back. The field
        /// measured `avg_filtered` 7.4 against `avg_high_limit` 7.4 — the buffer
        /// parked exactly on its own trigger point.
        #[test]
        fn a_small_target_should_not_park_the_buffer_above_its_high_limit() {
            for target in 5..=64 {
                let Band { low, high } = TargetController::buffer_limits(target);
                assert_eq!(
                    high,
                    target,
                    "at target {target} the drain must arm at the target itself, \
                     not {} frames above it",
                    high - target,
                );
                assert!(low < high, "the band must stay open at target {target}");
            }
        }

        /// The rebuffer resume clamp rests on this. It reads the band at
        /// `max(target, raw_target)` rather than at `target`, and the argument that
        /// this can only ever *retain* more audio than clamping on `target` alone —
        /// never less — is exactly the claim that `high` never falls as the target
        /// rises.
        ///
        /// True by construction: `high = max(t, floor(3t/4) + MIN_BAND)`, a max of
        /// two non-decreasing functions of `t`. Swept anyway, across every target
        /// any profile can reach, because the failure mode if it ever inverted is
        /// silent — the clamp would discard audio the buffer had just been measured
        /// to need, and the discard is spliced rather than logged as a fault.
        #[test]
        fn the_high_limit_must_never_fall_as_the_target_rises() {
            // 0 is the no-buffer sentinel; 100 frames is the largest comfort cap any
            // profile carries (Unknown, 1000ms), swept well past it for margin.
            let mut prev = TargetController::buffer_limits(0).high;
            for target in 1..=200u32 {
                let Band { low, high } = TargetController::buffer_limits(target);
                assert!(
                    high >= prev,
                    "high fell from {prev} to {high} between target {} and \
                     {target} — the resume clamp would discard below the depth \
                     the buffer was measured to need here",
                    target - 1,
                );
                assert!(
                    low <= high,
                    "the band inverted at target {target}: ({low}, {high})",
                );
                prev = high;
            }
        }
    }

    /// The dwell is asymmetric: a target may rise on 150-200ms of evidence but may
    /// only fall on ~2s of it.
    ///
    /// The field captures measured the reason. On 2.4GHz one `GAP_STALE_DECAY` step
    /// moves the raw target by `0.1875 * max_gap` = 4.2/4.6 frames against a
    /// 3-frame dead-zone, so **81%/84%** of windows clear it on ageing alone — the
    /// target committed downward on no new evidence at all, and 60%/75% of those
    /// descents reversed within 20s while carrying 66%/75% of every accelerate
    /// splice. On 5GHz the same step is 0.52-0.75 of the dead-zone, which is why
    /// 5GHz never had the defect and why the fix is a dwell rather than a wider
    /// band. See [`DOWN_DWELL_MS`].
    mod the_target_may_fall_only_on_sustained_evidence {
        use super::*;

        /// `cfg()` is the 2.4GHz shape: cap 80 frames, quantum 4, hysteresis 3,
        /// `adaptive_dwell` 15, `down_dwell` 200, min_depth 3. Asserted rather than
        /// assumed, because every arithmetic constant below is derived from them.
        fn constants() -> (JitterConfig, u32, u32, u32) {
            let config = cfg();
            let up = TargetController::adaptive_dwell(&config);
            let down = TargetController::down_dwell();
            let min_depth = TargetController::min_depth_frames(&config);
            assert_eq!((up, down, min_depth), (15, 200, 3));
            assert_eq!(TargetController::adaptive_hysteresis(&config), 3);
            assert_eq!(TargetController::quantize_target(30, 4), 32);
            (config, up, down, min_depth)
        }

        /// A virgin `JitterStats` has `clean_streak == 0`, so `stability_ratio()`
        /// is 0.0 and the downward-probe branch is inert. That is what lets these
        /// tests attribute every movement to the dwell path.
        fn quiet_stats() -> JitterStats {
            let stats = JitterStats::new();
            assert!(
                stats.stability_ratio() <= 0.2,
                "precondition: the probe branch must be inert, or a descent here \
                 is not attributable to the dwell",
            );
            stats
        }

        /// Nothing may slow a *rising* target: a worsening link still has to be
        /// answered inside `adaptive_dwell`. Asserted from rest, and again
        /// immediately after a long run of falling evidence — the two directions
        /// must not pool their dwell counters.
        #[test]
        fn a_rising_target_should_still_commit_within_the_configured_dwell() {
            let (config, up, down, min_depth) = constants();
            let stats = quiet_stats();
            let now = Instant::now();

            // From rest.
            let mut control = TargetController::new();
            for _ in 0..up - 1 {
                control.advance(&config, &stats, 40, min_depth, now);
            }
            assert_eq!(
                control.ramp_goal, 2,
                "the goal must not move before the dwell elapses",
            );
            control.advance(&config, &stats, 40, min_depth, now);
            assert_eq!(
                control.ramp_goal, 40,
                "a rising target must commit on exactly {up} callbacks, as it did \
                 before the dwell became asymmetric",
            );

            // After a long run of falling evidence. Without the direction reset
            // this would inherit 199 callbacks of the wrong sign and commit on the
            // first rising one.
            let mut control = TargetController::new();
            control.effective_target = 40;
            control.ramp_goal = 40;
            for _ in 0..down - 1 {
                control.advance(&config, &stats, 30, min_depth, now);
            }
            assert_eq!(control.target_exit_count, down - 1);
            for i in 0..up - 1 {
                control.advance(&config, &stats, 60, min_depth, now);
                assert_eq!(
                    control.ramp_goal, 40,
                    "callback {i} after the reversal committed early — the falling \
                     run must not count toward a rising transition",
                );
            }
            control.advance(&config, &stats, 60, min_depth, now);
            assert_eq!(
                control.ramp_goal, 60,
                "the reversal must still commit within its own dwell",
            );
        }

        /// The defect itself. One decay step clears the dead-zone on 2.4GHz, and
        /// under the old symmetric dwell that alone committed a descent 150ms later.
        #[test]
        fn a_falling_target_should_not_commit_on_a_single_decay_step() {
            let (config, up, down, min_depth) = constants();
            let stats = quiet_stats();
            let now = Instant::now();
            let mut control = TargetController::new();
            control.effective_target = 40;
            control.ramp_goal = 40;

            // 30 frames is 40 minus one decay step's worth on this link
            // (0.1875 * max_gap with max_gap ~ 50), quantizing to 32 — 8 frames
            // out, well clear of the 3-frame dead-zone.
            for _ in 0..down - 1 {
                control.advance(&config, &stats, 30, min_depth, now);
            }
            assert!(
                down - 1 > up,
                "precondition: the symmetric dwell ({up}) must be satisfied many \
                 times over inside this window, or the test proves nothing",
            );
            assert_eq!(
                (control.ramp_goal, control.effective_target),
                (40, 40),
                "the target moved on {} callbacks of evidence; a descent requires {down}",
                down - 1,
            );

            // ...and the very next one commits, so this is a delay and not a veto.
            control.advance(&config, &stats, 30, min_depth, now);
            assert_eq!(
                control.ramp_goal, 32,
                "the descent must commit the moment the dwell is satisfied",
            );
        }

        /// The delay must not become a floor: a link that genuinely improves still
        /// hands the latency back, just ~2s per quantum later.
        #[test]
        fn a_sustained_improvement_should_still_walk_the_target_down() {
            let (config, _, down, min_depth) = constants();
            let stats = quiet_stats();
            let now = Instant::now();
            let mut control = TargetController::new();
            control.effective_target = 40;
            control.ramp_goal = 40;

            // The dwell, plus the rate-limited ramp that follows it: 8 frames at
            // one frame per RAMP_INTERVAL callbacks.
            let budget = down + 8 * RAMP_INTERVAL + 10;
            for _ in 0..budget {
                control.advance(&config, &stats, 30, min_depth, now);
            }
            assert_eq!(
                control.effective_target, 32,
                "a sustained improvement must still reach the quantized target",
            );
            assert_eq!(control.ramp_goal, 32);

            // And it settles there rather than drifting: at the goal the dwell
            // branch is in its dead-zone and the probe branch is inert.
            for _ in 0..down * 2 {
                control.advance(&config, &stats, 30, min_depth, now);
            }
            assert_eq!(control.effective_target, 32);
        }

        /// The whole point of 2s. The measured age-reset period between gaps is
        /// p50 = 5.4-6.6s against a flat-top of 8s, so on 2.4GHz the next gap
        /// re-arms the target before a descent that began at the flat-top edge can
        /// commit. Under the symmetric dwell the descent committed 150ms in and then
        /// reversed — 400/310 frames shed against a net drift of 2.4/3.8 frames.
        #[test]
        fn a_gap_recurring_inside_the_down_dwell_should_leave_the_target_untouched() {
            let (config, _, down, min_depth) = constants();
            let stats = quiet_stats();
            let now = Instant::now();
            let mut control = TargetController::new();
            control.effective_target = 40;
            control.ramp_goal = 40;

            // Five cycles of "decay for 1s, then a fresh gap restores the raw
            // target". Total evidence is 2.5x the dwell; none of it is consecutive.
            let mut callbacks = 0;
            for _ in 0..5 {
                for _ in 0..down / 2 {
                    control.advance(&config, &stats, 30, min_depth, now);
                    callbacks += 1;
                }
                control.advance(&config, &stats, 40, min_depth, now);
                callbacks += 1;
                assert_eq!(
                    control.target_exit_count, 0,
                    "a gap landing back inside the dead-zone must restart the dwell",
                );
            }
            assert!(
                callbacks > down,
                "precondition: the run must be longer than the dwell ({callbacks} \
                 vs {down}), or nothing was held off",
            );
            assert_eq!(
                (control.ramp_goal, control.effective_target),
                (40, 40),
                "an oscillating link must not shed a single frame",
            );
        }
    }
}
