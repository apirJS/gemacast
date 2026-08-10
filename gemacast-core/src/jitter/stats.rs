//! Network-observation actor: consumes packet arrival times and maintains the
//! jitter statistics (dual-EMA jitter, variance, clean streak, NetEQ
//! relative-arrival-delay histogram, burst clustering, sliding-window max
//! delivery gap) that the [`super::target::TargetController`] reads to size the
//! buffer. Owns no buffer or decoder — it only observes.
//!
//! Invariant: **no statistic here may latch its own history.** Every depth
//! signal must fall on its own once the link improves, either by ageing out of a
//! window or by being recomputed from a bounded history. Peak-latches
//! (`ema_peak`, `max_iat_cumulative_sum`) used to live here and were the direct
//! cause of a target pinned at the comfort cap while the honest gap signal read
//! 10 frames.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use super::consts::{MILLIS_PER_FRAME, lerp};

/// Max inter-arrival time (ms) to consider two packets part of the same cluster.
/// Wi-Fi power-save delivers batched packets back-to-back with near-zero spacing;
/// 3ms absorbs jitter without merging distinct clusters.
const CLUSTER_IAT_THRESHOLD_MS: f32 = 3.0;
/// Min gap (ms) after the last cluster packet to declare the cluster ended.
/// A DTIM batch is followed by silence until the next beacon interval; 30ms
/// is well above intra-cluster spacing but below any realistic DTIM period.
const CLUSTER_GAP_THRESHOLD_MS: f32 = 30.0;
/// Minimum packets in a cluster for it to count. Single-packet "clusters" are
/// just normal traffic, and two-packet clusters are ambiguous. Three-plus is a
/// strong signal of batched delivery.
const MIN_CLUSTER_SIZE: u32 = 3;
/// Seconds of no detected clusters before clearing `burst_detected`. Keeps the
/// burst flag active through short clean intervals between DTIM windows.
const BURST_EXPIRY_SECS: u64 = 5;

/// NetEQ `kMaxHistoryPackets` (`delay_manager.cc:40`): how many packets the
/// relative-arrival-delay window looks back. Relative delay is measured against
/// the packet *preceding* this window, which bounds reference staleness under
/// clock drift — an unbounded reference would slowly diverge from the DAC clock.
const DELAY_HISTORY_PACKETS: usize = 100;

/// Bins in the relative-arrival-delay histogram, one frame wide each — 650ms of
/// range at 10ms frames.
///
/// Deliberately narrower than NetEQ's 100 buckets × 20ms (2000ms). NetEQ's
/// histogram is the sole memory of rare delay events and must span them; ours
/// shares that job with [`JitterStats::max_gap_frames`], which owns rare events
/// on an explicit 8s-fresh / 24s-maximum horizon. A delay this range cannot
/// represent is therefore *discarded* rather than folded into the top bin — see
/// [`JitterStats::histogram_bin`]. Widening to NetEQ's geometry was modelled
/// against this exact Q30 arithmetic and produced a *higher* target (67-69
/// frames vs 64), because the wider range tracks out-of-range delays honestly
/// instead of declining to represent them.
const IAT_HISTOGRAM_BINS: usize = 65;

/// Seconds of history in the delivery-gap window. The buffer must survive the
/// worst gap in this window; anything older is no longer representative of the
/// link and should be forgotten so the target can descend.
const GAP_WINDOW_BUCKETS: usize = 24;
/// Buckets younger than this are honoured at full value: a gap that just
/// happened is still "the link right now" and must be fully covered.
///
/// **This value has been 8 → 3 → 8, and the round trip is the evidence.**
///
/// v13 shortened it to 3s on the argument that a *recurring* gap is protected by
/// refresh (see [`JitterStats::max_gap_frames`]), so the flat-top could only ever
/// extend the cost of the non-recurring ones. The argument is sound; the premise
/// was not. It assumed gap recurrence is either sub-second (2.4GHz DTIM) or a
/// one-off, and nothing in between.
///
/// [`JitterStats::max_gap_age_secs`] was added in the same round to settle that
/// question, and the v13 capture answers it — recurrence is neither:
///
/// | link | age resets | mean recurrence | landing in (3,8] s | cover ceded |
/// | --- | --- | --- | --- | --- |
/// | 5GHz | 87 | **4.77s** | 47 (**54%**) | 2.66 fr (**27ms**) |
/// | ADB | 69 | **3.57s** | 38 (**55%**) | 0.80 fr (8ms) |
/// | 2.4GHz | 43 | **4.86s** | 24 (**56%**) | 2.99 fr (**30ms**) |
///
/// Over half of every link's recurrences land in the exact band 3s stopped
/// covering, so `max_gap` decayed between recurrences and the target sawtoothed
/// under the link's real need. The age histogram drops sharply at age 4 on all
/// three links — the fingerprint of the 3s flat-top cutting recurrences in half.
/// 5GHz paid for it directly: target 12.74 → 8.75 frames while concealment
/// windows rose 8 → 27.
///
/// Under 8s a 2-5s recurrence is never discounted at all, which is the behaviour
/// the mechanism's own doc describes. It also means v12's 73s 5GHz plateau —
/// which v13 read as a one-off riding an over-long flat-top — was correct
/// behaviour for a genuinely recurring gap.
///
/// This *raises* the target by ~30ms on the two wireless links and ~8ms on ADB.
/// Paid deliberately: cover the gap, pay the latency. It is only affordable
/// because the drain and the underrun defence were repaired first — under v13's
/// inert actuator the same 30ms would have been a pure tax with no way back down.
///
/// The decay past the flat-top and the 24s window are unchanged, so a true
/// one-off still glides off in ~20s rather than sitting for the window's life.
const GAP_FRESH_SECS: usize = 8;
/// Per-second decay applied to buckets older than [`GAP_FRESH_SECS`].
/// 0.85^20 ≈ 0.04 by the time a bucket rotates out at 24s, so a one-off spike
/// becomes a smooth glide down instead of a 24-second cliff (the field log shows
/// a single 1.07s gap pinning latency at ~730ms for the window's whole life).
/// Recurring gaps are unaffected — they keep refreshing young buckets.
const GAP_STALE_DECAY: f32 = 0.85;
/// Ignore gaps beyond 1.2s. Those are outages handled by the stream-reset path
/// (`max_missing_for`), not something a comfort buffer should size itself against.
const GAP_CLAMP_FRAMES: f32 = 120.0;

/// Rolling network-condition statistics derived from packet inter-arrival times.
pub(super) struct JitterStats {
    /// EWMA of inter-arrival jitter (frames).
    pub ema_jitter: f32,
    /// EWMA of jitter² for variance tracking.
    /// Combined with `ema_jitter`, yields coefficient of variation (CV = σ/μ)
    /// to distinguish stable-low-jitter from spiky-bursty networks.
    pub ema_jitter_var: f32,
    /// Consecutive packets with jitter below the adaptive clean threshold.
    /// Used to infer network quality: high streak = stable link.
    pub clean_streak: u32,
    /// When the last major jitter spike (>50ms) occurred.
    last_macro_spike: Option<Instant>,
    /// Unstable network (e.g. 2.4GHz scan cycle) regime expiration.
    unstable_regime_until: Option<Instant>,
    /// Last ingested sequence number to detect consecutive packets for IAT.
    last_ingest_seq: Option<u64>,
    /// Wall-clock arrival of the last forward packet, for IAT computation.
    last_network_arrival: Option<Instant>,
    // --- NetEQ relative-arrival-delay histogram ---
    /// Per-packet signed IAT excess (frames) over the last
    /// [`DELAY_HISTORY_PACKETS`] arrivals. The running sum floored at zero is
    /// NetEQ's relative arrival delay (`CalculateRelativePacketArrivalDelay`).
    delay_history: VecDeque<f32>,
    /// Q30 exponential-forgetting histogram of relative arrival delay, one bin
    /// per frame — [`IAT_HISTOGRAM_BINS`] bins therefore span 650ms.
    /// iat_histogram[i] = probability mass that the delay == i frames (Q30).
    iat_histogram: [i64; IAT_HISTOGRAM_BINS],
    /// Forgetting-factor convergence counter. Starts at 0, converges toward
    /// `IAT_FACTOR_STEADY` (≈0.9993 in Q15) over the first ~1000 packets.
    iat_factor: i64,
    /// 95th-percentile relative delay in frames, derived from the histogram.
    pub iat_percentile_target: f32,
    // --- Burst Cluster Detection ---
    /// Number of packets in the current cluster (reset when a gap exceeds
    /// CLUSTER_GAP_THRESHOLD_MS).
    cluster_packet_count: u32,
    /// Wall-clock arrival time of the first packet in the current cluster.
    cluster_start_time: Option<Instant>,
    /// Start time of the previous completed cluster, used to compute the
    /// inter-burst gap.
    last_cluster_start: Option<Instant>,
    /// EWMA of inter-burst gap in frames. This is the core signal for
    /// the 5GHz screen-off fix: it directly measures the DTIM batching period.
    inter_burst_gap_frames: f32,
    /// True when active burst clustering is detected. Set when a qualifying
    /// cluster completes; cleared after BURST_EXPIRY_SECS of no clusters.
    burst_detected: bool,
    /// When the last qualifying cluster was detected. Used for expiry.
    last_burst_time: Option<Instant>,
    // --- Sliding-window max delivery gap ---
    /// Ring of per-second maxima of the observed delivery gap, in frames.
    gap_buckets: [f32; GAP_WINDOW_BUCKETS],
    /// Index of the bucket currently accumulating.
    gap_bucket_idx: usize,
    /// Arrival time that opened the current bucket.
    gap_bucket_start: Option<Instant>,
    /// Packets accepted by `observe` since the last observability log line.
    /// Drained by [`Self::take_arrival_count`]; carries no decision authority.
    arrival_count: u32,
}

impl JitterStats {
    pub fn new() -> Self {
        let mut s = Self {
            ema_jitter: 0.0,
            ema_jitter_var: 0.0,
            clean_streak: 0,
            last_macro_spike: None,
            unstable_regime_until: None,
            last_ingest_seq: None,
            last_network_arrival: None,
            delay_history: VecDeque::with_capacity(DELAY_HISTORY_PACKETS),
            iat_histogram: [0i64; IAT_HISTOGRAM_BINS],
            iat_factor: 0,
            iat_percentile_target: 0.0,
            cluster_packet_count: 0,
            cluster_start_time: None,
            last_cluster_start: None,
            inter_burst_gap_frames: 0.0,
            burst_detected: false,
            last_burst_time: None,
            gap_buckets: [0.0; GAP_WINDOW_BUCKETS],
            gap_bucket_idx: 0,
            gap_bucket_start: None,
            arrival_count: 0,
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

    /// NetEQ `DelayManager::UpdateDelayHistory` (`delay_manager.cc:275-280`).
    /// Push one packet's signed IAT excess onto the bounded history.
    fn update_delay_history(&mut self, iat_delay: f32) {
        self.delay_history.push_back(iat_delay);
        if self.delay_history.len() > DELAY_HISTORY_PACKETS {
            self.delay_history.pop_front();
        }
    }

    /// NetEQ `DelayManager::CalculateRelativePacketArrivalDelay`
    /// (`delay_manager.cc:282-293`).
    ///
    /// Arrival delay of the newest packet relative to the packet preceding the
    /// history window. The floor-at-zero inside the loop is what moves the
    /// reference forward: once the link has caught up, the accumulated lateness
    /// is discarded rather than remembered, so this can never latch.
    fn relative_packet_arrival_delay(&self) -> f32 {
        let mut relative_delay = 0.0f32;
        for delay in &self.delay_history {
            relative_delay = (relative_delay + delay).max(0.0);
        }
        relative_delay
    }

    /// NetEQ `DelayManager::UpdateHistogram` + `CalculateTargetLevel` port.
    ///
    /// Feeds the observed **relative arrival delay** (in whole frames) into a
    /// 65-bin Q30 exponential-forgetting histogram, then walks the reverse CDF to
    /// find the 95th percentile. Result is stored in `iat_percentile_target`
    /// (frames). Mirrors `delay_manager.cc:239-247` (RELATIVE_ARRIVAL_DELAY mode)
    /// + `histogram.cc` forgetting/quantile math.
    fn update_iat_histogram(&mut self, delay_frames: f32) {
        /// Exponential-forgetting factor in Q15: ≈0.99827, a ~400-update
        /// half-life (~4s at our 100 packets/s).
        ///
        /// NetEQ ships 32745 (~990 updates ≈ 20s at its 20ms frames, ~10s at our
        /// 10ms ones). We deliberately forget faster, because NetEQ's histogram is
        /// the *sole* memory of rare delay events and must therefore be long, while
        /// ours is not: [`JitterStats::max_gap_frames`] owns rare events explicitly,
        /// with a known 8s-fresh / 24s-maximum horizon. Leaving the histogram at
        /// NetEQ's half-life made it — not the gap window — the slowest term in the
        /// descent: 4 seconds of DTIM batching still held p95 at ~31 frames a full
        /// 22s after the link went clean, which blows the screen-on snap-back
        /// budget. At a 4s half-life the histogram has shed 95% of an episode in
        /// ~17s, comfortably inside the window's own horizon, so neither signal can
        /// be the one that pins the target.
        const IAT_FACTOR_STEADY: i64 = 32711;
        /// 5% tail probability in Q30 → 95th percentile.
        const LIMIT_PROBABILITY: i64 = (1 << 30) / 20;

        // A delay past the histogram's range is DISCARDED, not clamped into the
        // top bin — `delay_manager.cc:241-247` does the same, and this module's
        // no-self-latching invariant requires it.
        //
        // Clamping was the v12 2.4GHz silence. `relative_packet_arrival_delay` is
        // a running sum floored at zero, so sustained lateness carries it well
        // past this histogram's 65-frame span; every such sample then refreshed
        // the ceiling bin, and a bin that is refreshed on every out-of-range
        // observation is a bin the reverse-CDF walk can never fall off. The field
        // log shows the result exactly: `histogram` stepped 26.0 → 63.0 while
        // `max_gap` held flat at 30.3 and `gap_floor` *fell* 38.9 → 31.3, then
        // read 63.0 — the walk's maximum, not a measurement — for 22 consecutive
        // windows (~70s) with occupancy repeatedly at zero. Modelled against this
        // exact Q30 arithmetic the latch takes 0.5s of out-of-range delay to form
        // and 12.7s of a perfectly clean link to shed: a 25:1 asymmetry no real
        // 2.4GHz clean stretch was long enough to pay off.
        //
        // Discarding is not a loss of coverage, because the histogram was never
        // the term that owns these events. The doc above says so in as many
        // words: [`JitterStats::max_gap_frames`] owns rare events, with its own
        // 8s-fresh / 24s-maximum horizon, and `gap_floor` is what turns them into
        // depth. What the histogram owns is the *body* of the delay distribution,
        // and a sample it cannot represent tells it nothing about that body.
        //
        // Widening the range instead was modelled and rejected: NetEQ's geometry
        // (100 buckets × 20ms) brings a 700ms delay back *in* range and tracks it
        // honestly to 67-69 frames, worse than the 64 we have. NetEQ is right to
        // do that — its histogram is the sole memory of rare events. Copying its
        // geometry without copying that responsibility split is the trap.
        let Some(iat_packets) = Self::histogram_bin(delay_frames) else {
            return;
        };

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
        // Bounded by the array, not by a literal — `histogram.cc:99` walks to
        // `buckets_.size() - 1` for the same reason. Unreachable now that the top
        // bin takes no samples, but 63.0 is the number that cost the last field
        // round and a hand-written bound is how it became plausible.
        let last_bin = self.iat_histogram.len() - 1;
        let mut sum: i64 = (1 << 30) - self.iat_histogram[0];
        let mut index = 0usize;
        while sum > LIMIT_PROBABILITY && index < last_bin {
            index += 1;
            sum -= self.iat_histogram[index];
        }
        self.iat_percentile_target = index as f32;
    }

    /// Histogram bin for a relative arrival delay, or `None` when the delay is
    /// outside the range the histogram can represent.
    ///
    /// Split out so the discard is a named, testable decision rather than an
    /// expression inside the update. See [`JitterStats::update_iat_histogram`]
    /// for why out-of-range samples must not be folded into the top bin.
    ///
    /// Negatives are discarded rather than floored to bin 0. The only caller
    /// feeds [`JitterStats::relative_packet_arrival_delay`], which is floored at
    /// zero already, so this cannot fire today — but "a delay this histogram
    /// cannot represent leaves it untouched" is the invariant, and flooring is
    /// the same mistake as clamping pointed the other way.
    fn histogram_bin(delay_frames: f32) -> Option<usize> {
        if !(0.0..IAT_HISTOGRAM_BINS as f32).contains(&delay_frames) {
            return None;
        }
        Some(delay_frames as usize)
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

    /// Expose the unstable-regime deadline for the target controller's probe gate.
    pub fn unstable_regime_until(&self) -> Option<Instant> {
        self.unstable_regime_until
    }

    /// Whether burst cluster detection is currently active (Wi-Fi power-save
    /// batching pattern detected within the last BURST_EXPIRY_SECS).
    pub fn burst_detected(&self) -> bool {
        self.burst_detected
    }

    /// EWMA of the inter-burst gap in frames. Represents the DTIM batching
    /// period: 100ms gap → 10 frames, 200ms → 20 frames. Only meaningful
    /// when `burst_detected()` is true.
    ///
    /// **Not a depth authority.** v11 measured this at 5783.5 frames (57.8s) on a
    /// USB cable while the honest `max_gap_frames()` on the same capture peaked at
    /// 24 — see the invariant comment in [`super::target`]. It is reported in the
    /// depth log and it gates `gap_floor`'s headroom; it does not set depth.
    pub fn inter_burst_gap_frames(&self) -> f32 {
        self.inter_burst_gap_frames
    }

    /// Test seam: put the burst detector into a chosen state directly.
    ///
    /// Exists because the two signals this module exposes — `inter_burst_gap` and
    /// `max_gap` — can only be made to *disagree* through a stale cluster anchor or
    /// through their different ageing rules, both of which are `stats` concerns
    /// tested here. [`super::target`] needs to assert what the depth authority does
    /// with a disagreement it did not cause, so it is handed one.
    #[cfg(test)]
    pub(super) fn force_burst_state(&mut self, detected: bool, inter_burst_gap_frames: f32) {
        self.burst_detected = detected;
        self.inter_burst_gap_frames = inter_burst_gap_frames;
    }

    /// Fold one observed delivery gap into the sliding window.
    ///
    /// Rotation is driven by packet arrivals rather than a timer, so a long
    /// silence cannot rotate the window away *while* the gap is happening — the
    /// gap is recorded by the packet that ends it, into a bucket that is still
    /// current at that moment.
    /// Record one observed delivery gap into the sliding window. `pub(super)` so
    /// [`super::target`] tests can populate the window in isolation — the whole
    /// point of this signal is that it works when every other statistic is silent.
    pub(super) fn record_gap(&mut self, gap_frames: f32, arrival_time: Instant) {
        let gap = gap_frames.clamp(0.0, GAP_CLAMP_FRAMES);
        match self.gap_bucket_start {
            None => {
                self.gap_bucket_start = Some(arrival_time);
            }
            Some(start) => {
                let elapsed_secs = arrival_time.duration_since(start).as_secs() as usize;
                if elapsed_secs >= 1 {
                    // Zero every bucket we passed through, including any that
                    // elapsed with no arrivals at all, so stale maxima cannot
                    // survive a quiet stretch.
                    let steps = elapsed_secs.min(GAP_WINDOW_BUCKETS);
                    for _ in 0..steps {
                        self.gap_bucket_idx = (self.gap_bucket_idx + 1) % GAP_WINDOW_BUCKETS;
                        self.gap_buckets[self.gap_bucket_idx] = 0.0;
                    }
                    self.gap_bucket_start = Some(start + Duration::from_secs(elapsed_secs as u64));
                }
            }
        }
        let slot = &mut self.gap_buckets[self.gap_bucket_idx];
        *slot = slot.max(gap);
    }

    /// Largest delivery gap observed in the last [`GAP_WINDOW_BUCKETS`] seconds,
    /// in frames, **weighted down by age**.
    ///
    /// This is the primary depth signal. A buffer shallower than the largest
    /// delivery gap *will* run dry — that is the entire mechanism of starvation.
    /// Unlike [`Self::iat_percentile_target`] it is not diluted by the dense
    /// burst that follows every DTIM gap: with ~50 packets per burst a 500ms gap
    /// sits at the 98th percentile and is invisible to a p95-of-packets
    /// statistic, which is why the old algorithm could only learn a gap by
    /// starving on it.
    ///
    /// It rises within one packet of a new worst gap and falls purely by ageing
    /// — no ratchet, no latch. Buckets younger than [`GAP_FRESH_SECS`] count at
    /// full value; beyond that each further second multiplies by
    /// [`GAP_STALE_DECAY`], so a one-off spike glides down from the flat-top's
    /// edge instead of falling off a cliff at +24s. A *recurring* gap keeps
    /// refreshing young buckets and is therefore unaffected by the weighting —
    /// which is why the flat-top has to be at least as long as the link's real
    /// recurrence period. Measured, that period is 3.6-4.9s on every link, so a
    /// flat-top shorter than that discounts gaps that are still happening.
    pub fn max_gap_frames(&self) -> f32 {
        self.weighted_gap_peak().0
    }

    /// Age in seconds of the bucket currently producing [`Self::max_gap_frames`]
    /// — i.e. how long ago the gap that is setting the depth target happened.
    ///
    /// Reported on the 1Hz depth line only. It exists because the v12 captures
    /// cannot distinguish the two ways `max_gap` holds a level, and those two
    /// have opposite meanings:
    ///
    /// - **Flat-top**: one gap, aged under [`GAP_FRESH_SECS`], honoured at full
    ///   value. Age climbs 0 → [`GAP_FRESH_SECS`] and the level then decays.
    /// - **Refresh**: the gap keeps recurring, each occurrence writing a fresh
    ///   young bucket. Age resets toward 0 every period.
    ///
    /// A 1Hz log of the *level* alone reads identically in both cases, which is
    /// why the 5GHz plateau at L713→L959 (73s flat at ~21 frames, `arrivals`
    /// 99-104/s) could not be attributed. Age makes the recurrence period
    /// directly readable — and reading it is what settled [`GAP_FRESH_SECS`]:
    /// 87 / 69 / 43 age resets across the three links, mean period 4.77 / 3.57 /
    /// 4.86s, which is neither the sub-second refresh nor the one-off that a 3s
    /// flat-top assumed. That plateau was refresh-sustained after all.
    pub fn max_gap_age_secs(&self) -> usize {
        self.weighted_gap_peak().1
    }

    /// The age-weighted peak and the age of the bucket that produced it.
    /// Shared so the logged age can never disagree with the logged level.
    fn weighted_gap_peak(&self) -> (f32, usize) {
        let n = GAP_WINDOW_BUCKETS;
        let mut max = 0.0f32;
        let mut winner = 0usize;
        for (i, gap) in self.gap_buckets.iter().enumerate() {
            if *gap <= 0.0 {
                continue;
            }
            // The currently-accumulating bucket is age 0; the one about to be
            // recycled is age N-1.
            let age = (self.gap_bucket_idx + n - i) % n;
            let weighted = if age <= GAP_FRESH_SECS {
                *gap
            } else {
                *gap * GAP_STALE_DECAY.powi((age - GAP_FRESH_SECS) as i32)
            };
            // Ties resolve to the *younger* bucket: a recurring gap writes the
            // same value into successive buckets, and the recent one is the
            // honest answer to "when did the gap that is setting depth happen".
            if weighted > max || (weighted == max && age < winner) {
                max = weighted;
                winner = age;
            }
        }
        (max, winner)
    }

    /// Clear the delivery-gap window. Used by both reset paths.
    fn reset_gap_window(&mut self) {
        self.gap_buckets = [0.0; GAP_WINDOW_BUCKETS];
        self.gap_bucket_idx = 0;
        self.gap_bucket_start = None;
    }

    /// Observe a single forward packet's arrival, updating all jitter statistics.
    ///
    /// Returns `true` if the caller should insert the packet into the reorder
    /// buffer, or `false` to drop it (clock ran backwards).
    pub fn observe(&mut self, seq_num: u64, arrival_time: Instant) -> bool {
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

                let iat_packets = iat_actual / MILLIS_PER_FRAME as f32;

                // --- NetEQ relative-arrival-delay histogram update ---
                // Port of `delay_manager.cc:229-236 + 275-293`. The per-packet
                // signed IAT excess goes into a 100-packet history; the histogram
                // is then fed the running sum of that history floored at zero.
                //
                // Why the running sum and not the per-packet excess: after a
                // 500ms DTIM gap, *every* packet of the burst that follows is
                // still late relative to the packet before the gap, so one gap
                // contributes ~50 large samples instead of a single one. The old
                // per-packet feeding buried the gap at the 98th percentile of a
                // burst-dominated sample set — structurally invisible at p95,
                // which is why gaps could only ever be learned by starving.
                let iat_delay = iat_packets - seq_diff as f32;
                self.update_delay_history(iat_delay);
                let relative_delay = self.relative_packet_arrival_delay();
                self.update_iat_histogram(relative_delay);

                // --- Sliding-window max delivery gap ---
                // Deliberately the RAW gap, not `iat_adjusted`: lost packets
                // never feed the buffer, so the wall-clock silence the playhead
                // had to survive is exactly what a starvation-free depth must
                // cover. Subtracting the missing-packet count (correct for the
                // histogram, which models per-packet lateness) would understate
                // the depth requirement.
                self.record_gap(iat_packets, arrival_time);

                // --- Burst cluster detection ---
                // Detect Wi-Fi power-save batching: packets arrive in tight
                // clusters (< 3ms apart) separated by long gaps (DTIM interval).
                // All existing jitter signals are blind to this pattern because
                // per-packet jitter within each cluster is near-zero.
                //
                // Expiry runs FIRST, against the previous `last_burst_time`. It used
                // to run after the cluster block, which made it structurally unable
                // to fire on the packet that needs it most: a packet resuming after
                // a long silence closes a cluster, writes `last_burst_time = now`,
                // and the expiry check then sees a fresh timestamp and stands down.
                // That is how v11 measured a 57.8s "inter-burst gap" on a USB cable.
                if self.burst_detected
                    && let Some(last_burst) = self.last_burst_time
                    && arrival_time.duration_since(last_burst).as_secs() >= BURST_EXPIRY_SECS
                {
                    self.burst_detected = false;
                    self.inter_burst_gap_frames = 0.0;
                    // The anchor must go with them. Leaving it behind is what let the
                    // *next* cluster measure its gap against a cluster from an
                    // arbitrary distance in the past — `inter_burst_gap` then reports
                    // wall-clock time since the last coalescing event, which on a
                    // quiet link is unbounded, instead of a DTIM period.
                    self.last_cluster_start = None;
                    tracing::debug!("[JitterMgr] Burst detection expired");
                }

                if iat_actual < CLUSTER_IAT_THRESHOLD_MS {
                    // Packet arrived within the cluster threshold — extend or
                    // start a new cluster.
                    self.cluster_packet_count += 1;
                    if self.cluster_start_time.is_none() {
                        self.cluster_start_time = Some(arrival_time);
                    }
                } else if iat_actual > CLUSTER_GAP_THRESHOLD_MS {
                    // Gap exceeds cluster threshold — the previous cluster has ended.
                    if self.cluster_packet_count >= MIN_CLUSTER_SIZE {
                        // Valid cluster detected. Compute inter-burst gap if we
                        // have a prior cluster start to compare against.
                        if let Some(prev_start) = self.last_cluster_start
                            && let Some(cur_start) = self.cluster_start_time
                        {
                            let gap_ms =
                                cur_start.duration_since(prev_start).as_secs_f32() * 1000.0;
                            let gap_frames = gap_ms / MILLIS_PER_FRAME as f32;
                            // Reject anything wider than the delivery-gap window's own
                            // clamp. Beyond 1.2s it is an outage for the stream-reset
                            // path, not a batching period — and since the EWMA is
                            // neither aged nor bounded, one such sample would sit in
                            // the reported signal until the next expiry. The gap window
                            // rejects the same magnitude for the same reason
                            // (`GAP_CLAMP_FRAMES`); this is that rule applied to the
                            // one statistic that was exempt from it.
                            if gap_frames <= GAP_CLAMP_FRAMES {
                                // EWMA with α=0.3 — responsive to DTIM period changes
                                // but stable enough to ignore occasional timing jitter.
                                if self.inter_burst_gap_frames == 0.0 {
                                    self.inter_burst_gap_frames = gap_frames;
                                } else {
                                    self.inter_burst_gap_frames =
                                        self.inter_burst_gap_frames * 0.7 + gap_frames * 0.3;
                                }
                            } else {
                                tracing::debug!(
                                    "[JitterMgr] Inter-burst gap {:.1} frames exceeds the {:.0}-frame clamp — rejected, not a batching period",
                                    gap_frames,
                                    GAP_CLAMP_FRAMES,
                                );
                            }
                        }
                        self.last_cluster_start = self.cluster_start_time;
                        self.burst_detected = true;
                        self.last_burst_time = Some(arrival_time);
                        tracing::debug!(
                            "[JitterMgr] Burst cluster detected: {} packets, inter_burst_gap={:.1} frames",
                            self.cluster_packet_count,
                            self.inter_burst_gap_frames,
                        );
                    }
                    // Reset for the next cluster (this packet starts it).
                    self.cluster_packet_count = 1;
                    self.cluster_start_time = Some(arrival_time);
                } else {
                    // Mid-range gap (3-30ms): not tight enough for a cluster,
                    // not wide enough to end one. Reset cluster tracking.
                    self.cluster_packet_count = 0;
                    self.cluster_start_time = None;
                }

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

                // --- Macro-spike / unstable-regime tracking ---
                // The only consumer left is the target controller's probe gate:
                // while the regime is unstable we refuse to probe the buffer
                // downward. Depth itself is sized by the gap window and the
                // relative-delay histogram — never by a latched spike history.
                if jitter_frames >= 10.0 {
                    // Spikes > 50ms (10 frames).
                    let mut is_new_macro_spike = false;
                    if let Some(last_spike) = self.last_macro_spike {
                        let interval = arrival_time.duration_since(last_spike).as_millis();
                        if interval > 500 {
                            // Debounce burst packets
                            is_new_macro_spike = true;
                            // If spikes are frequent (<10s), network is chronically poor
                            if interval < 10000 {
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
        }
        self.last_network_arrival = Some(arrival_time);
        self.last_ingest_seq = Some(seq_num);
        self.arrival_count = self.arrival_count.saturating_add(1);
        true
    }

    /// Read and clear the accepted-arrival counter.
    ///
    /// Observability only: paired with the manager's frames-played count it makes a
    /// delivery rate below the playback rate directly visible. Counts packets
    /// `observe` accepted, so a backwards-clock drop is excluded — the same
    /// population that feeds every statistic here.
    pub fn take_arrival_count(&mut self) -> u32 {
        std::mem::take(&mut self.arrival_count)
    }

    /// Partial reset on stream restart (matches the legacy `trigger_reset` field set):
    /// clears the jitter EMAs, streak and delay history but preserves the
    /// last network-arrival anchor.
    pub fn reset_on_stream_restart(&mut self) {
        self.ema_jitter = 0.0;
        self.last_ingest_seq = None;
        self.clean_streak = 0;
        self.ema_jitter_var = 0.0;
        self.delay_history.clear();
        self.reset_histogram_to_seed();
        self.cluster_packet_count = 0;
        self.cluster_start_time = None;
        self.last_cluster_start = None;
        self.inter_burst_gap_frames = 0.0;
        self.burst_detected = false;
        self.last_burst_time = None;
        self.reset_gap_window();
    }

    pub fn reset_on_config_change(&mut self) {
        self.ema_jitter = 0.0;
        self.clean_streak = 0;
        self.ema_jitter_var = 0.0;
        self.delay_history.clear();
        self.reset_histogram_to_seed();
        self.cluster_packet_count = 0;
        self.cluster_start_time = None;
        self.last_cluster_start = None;
        self.inter_burst_gap_frames = 0.0;
        self.burst_detected = false;
        self.last_burst_time = None;
        self.reset_gap_window();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Feed `secs` seconds of perfectly-spaced 10ms arrivals, advancing `seq`/`t`.
    fn feed_clean(stats: &mut JitterStats, seq: &mut u64, t: &mut Instant, secs: u64) {
        for _ in 0..secs * 100 {
            *t += Duration::from_millis(10);
            *seq += 1;
            stats.observe(*seq, *t);
        }
    }

    /// The sliding-window gap tracker is the load-bearing depth signal: it must
    /// learn a DTIM-sized delivery gap from a *single* arrival (no starvation
    /// needed) and then let go of it on its own. A ratchet here would reproduce
    /// the `probe_floor` bug one level down.
    #[test]
    fn max_gap_window_rises_on_one_gap_then_decays_back_to_zero() {
        let mut stats = JitterStats::new();
        let t0 = Instant::now();

        // Two back-to-back arrivals establish the baseline (10ms apart).
        stats.observe(1, t0);
        stats.observe(2, t0 + Duration::from_millis(10));
        assert!(
            stats.max_gap_frames() < 2.0,
            "clean 10ms spacing must not register a gap, got {:.1}",
            stats.max_gap_frames(),
        );

        // One 200ms DTIM gap, then the burst that follows it.
        let gap_end = t0 + Duration::from_millis(210);
        stats.observe(3, gap_end);
        assert!(
            (stats.max_gap_frames() - 20.0).abs() < 1.0,
            "a 200ms gap must be learned as ~20 frames immediately, got {:.1}",
            stats.max_gap_frames(),
        );

        // v5: the read is age-weighted, so the descent must already be well
        // underway at 16s — not a cliff at the 24s rotation boundary.
        let mut seq = 3u64;
        let mut t = gap_end;
        feed_clean(&mut stats, &mut seq, &mut t, 16);
        assert!(
            stats.max_gap_frames() < 10.0,
            "age-decay must have more than halved the gap by 16s, got {:.1}",
            stats.max_gap_frames(),
        );

        // And it must still rotate out entirely — no floor, no residue.
        feed_clean(&mut stats, &mut seq, &mut t, 10);
        assert!(
            stats.max_gap_frames() < 2.0,
            "gap must rotate out of the {}s window, got {:.1}",
            GAP_WINDOW_BUCKETS,
            stats.max_gap_frames(),
        );
    }

    /// v5 root cause 3: a single huge gap used to hold the target at full value
    /// for the window's entire 24s life (the field log shows one 1.07s gap
    /// pinning latency at ~730ms for ~50s). Age-weighting turns that cliff into
    /// a glide — while leaving a *recurring* gap fully covered, because it keeps
    /// refreshing young buckets.
    #[test]
    fn age_decay_glides_a_one_off_gap_but_holds_a_recurring_one() {
        // --- One-off: a 100-frame gap must fade as it ages. ---
        let mut stats = JitterStats::new();
        let t0 = Instant::now();
        stats.observe(1, t0);
        let mut t = t0 + Duration::from_secs(1);
        stats.observe(2, t);
        assert!(
            (stats.max_gap_frames() - 100.0).abs() < 1.0,
            "1s gap must read ~100 frames immediately, got {:.1}",
            stats.max_gap_frames(),
        );

        let mut seq = 2u64;
        feed_clean(&mut stats, &mut seq, &mut t, GAP_FRESH_SECS as u64);
        assert!(
            stats.max_gap_frames() > 95.0,
            "inside GAP_FRESH_SECS the gap must be honoured at full value, got {:.1}",
            stats.max_gap_frames(),
        );

        feed_clean(&mut stats, &mut seq, &mut t, 13); // age 16
        let at_16 = stats.max_gap_frames();
        assert!(
            (8.0..18.0).contains(&at_16),
            "at 16s a 100-frame one-off must have glided to ~12, got {at_16:.1}",
        );

        feed_clean(&mut stats, &mut seq, &mut t, 6); // age 22
        let at_22 = stats.max_gap_frames();
        assert!(
            at_22 < 6.0,
            "at 22s the one-off must be nearly gone, got {at_22:.1}",
        );

        // --- Recurring: a 20-frame gap every 2s must never be discounted. ---
        let mut stats = JitterStats::new();
        let t0 = Instant::now();
        stats.observe(1, t0);
        let mut seq = 1u64;
        let mut t = t0;
        for cycle in 0..10 {
            t += Duration::from_millis(200);
            seq += 1;
            stats.observe(seq, t);
            feed_clean(&mut stats, &mut seq, &mut t, 2);
            if cycle >= 1 {
                // Measured at the oldest point in the cycle — 2s after the last
                // gap, still inside GAP_FRESH_SECS.
                assert!(
                    stats.max_gap_frames() > 19.5,
                    "a gap recurring every 2s must stay fully covered, got {:.1} (cycle {cycle})",
                    stats.max_gap_frames(),
                );
            }
        }
    }

    /// The flat-top is the one thing in this mechanism that does not age, so both
    /// of its edges are asserted here and a future change to the constant has to
    /// come through this test and say so.
    ///
    /// The cost it is traded against is a one-off gap held longer than it
    /// deserves — the v12 5GHz capture shows a single 306ms spike at L579 holding
    /// `effective_target` at 32 frames for 8 consecutive log windows (L590→L611)
    /// while `arrivals` read 100/s and the histogram decayed 15 → 0. v13 answered
    /// that by shortening the flat-top and made the recurring case worse instead
    /// (see [`GAP_FRESH_SECS`]). The actual guarantee is weaker and is what this
    /// asserts: a one-off is honoured while fresh, takes its first decay step the
    /// second the flat-top ends, and is substantially gone well inside the 24s
    /// window — it is never pinned for the window's life, which was the v5 defect
    /// age-weighting exists to fix.
    #[test]
    fn a_one_off_gap_should_still_decay_within_the_window() {
        let mut stats = JitterStats::new();
        let t0 = Instant::now();
        stats.observe(1, t0);
        // One 300ms gap — the size the 5GHz capture actually produced.
        let mut t = t0 + Duration::from_millis(300);
        stats.observe(2, t);
        let mut seq = 2u64;

        feed_clean(&mut stats, &mut seq, &mut t, GAP_FRESH_SECS as u64);
        let at_fresh = stats.max_gap_frames();
        assert!(
            (at_fresh - 30.0).abs() < 0.5,
            "at exactly GAP_FRESH_SECS the gap is still 'the link right now' and must \
             read full value, got {at_fresh:.1}",
        );

        feed_clean(&mut stats, &mut seq, &mut t, 1);
        let after = stats.max_gap_frames();
        let expected = 30.0 * GAP_STALE_DECAY;
        assert!(
            (after - expected).abs() < 0.5,
            "one second past the flat-top the gap must have taken exactly one decay \
             step ({expected:.1} expected), got {after:.1}",
        );

        // Ten more seconds of a clean link: a *non*-recurring gap must be most of
        // the way gone, not riding a longer flat-top to the window's edge.
        feed_clean(&mut stats, &mut seq, &mut t, 10);
        let at_19 = stats.max_gap_frames();
        assert!(
            at_19 < 30.0 * 0.25,
            "11s past the flat-top a one-off must have glided to under a quarter of \
             its peak, got {at_19:.1}",
        );
    }

    /// **The measurement that set [`GAP_FRESH_SECS`], asserted directly.**
    ///
    /// The flat-top only has to be long enough to reach the link's real gap
    /// recurrence period; past that, refresh does the work. v13 assumed that
    /// period was either sub-second or nonexistent and shortened the flat-top to
    /// 3s. [`JitterStats::max_gap_age_secs`] then measured it: **4.77s on 5GHz,
    /// 3.57s on ADB, 4.86s on 2.4GHz**, with 54-56% of all recurrences landing in
    /// the (3, 8] band the shortened flat-top had stopped covering.
    ///
    /// Five seconds is therefore not an arbitrary period — it is the middle of the
    /// measured distribution on every link this ships to, and it must be covered
    /// at full weight. Under 3s it decayed to 0.85² ≈ 72% of the gap, which is the
    /// 27-30ms of ceded cover that took the 5GHz target from 12.74 to 8.75 frames
    /// while concealment windows rose 8 → 27.
    ///
    /// The far edge is asserted too, so this is a band and not a ratchet: a period
    /// well past the flat-top still decays between recurrences.
    #[test]
    fn a_gap_recurring_every_five_seconds_should_stay_at_full_weight() {
        /// A 20-frame gap every `period_secs`, read at the oldest point in the
        /// cycle — immediately before the next recurrence, where the weighting
        /// bites hardest.
        fn read_at_oldest_point(period_secs: u64) -> f32 {
            let mut stats = JitterStats::new();
            let t0 = Instant::now();
            stats.observe(1, t0);
            let mut seq = 1u64;
            let mut t = t0;
            let mut level = 0.0;
            for _ in 0..6 {
                t += Duration::from_millis(200);
                seq += 1;
                stats.observe(seq, t);
                feed_clean(&mut stats, &mut seq, &mut t, period_secs);
                level = stats.max_gap_frames();
            }
            level
        }

        let measured = read_at_oldest_point(5);
        assert!(
            measured > 19.5,
            "5s is the measured recurrence period on every link (4.77 / 3.57 / \
             4.86s) and must be held at full weight — got {measured:.1}, which is \
             the ceded cover that made the 5GHz target sawtooth under its need",
        );

        let at_edge = read_at_oldest_point(GAP_FRESH_SECS as u64);
        assert!(
            at_edge > 19.5,
            "a gap recurring exactly at the flat-top edge is still refresh-held and \
             must stay fully covered, got {at_edge:.1}",
        );

        // Far past the period any link actually shows: the weighting must still
        // bite, or the flat-top has become a ratchet.
        let stale_period = GAP_FRESH_SECS as u64 + 4;
        let ceded = read_at_oldest_point(stale_period);
        let expected = 20.0 * GAP_STALE_DECAY.powi((stale_period - GAP_FRESH_SECS as u64) as i32);
        assert!(
            (ceded - expected).abs() < 0.5,
            "a {stale_period}s period sits past the flat-top and must decay to \
             {expected:.1} at its oldest point — the flat-top covers the measured \
             recurrence band, it does not hold forever; got {ceded:.1}",
        );
    }

    /// The two ways `max_gap` holds a level read identically at 1Hz and mean
    /// opposite things, so the age that separates them is asserted directly.
    /// This is the signal the v12 5GHz capture lacked: 73s flat at ~21 frames
    /// (L713→L959) with `arrivals` at 99-104/s, unattributable either way.
    #[test]
    fn the_gap_age_should_separate_a_flat_top_from_a_refreshed_gap() {
        // --- One-off: age climbs, because nothing rewrites the bucket. ---
        let mut stats = JitterStats::new();
        let t0 = Instant::now();
        stats.observe(1, t0);
        let mut t = t0 + Duration::from_millis(300);
        stats.observe(2, t);
        let mut seq = 2u64;
        assert_eq!(stats.max_gap_age_secs(), 0, "a fresh gap is age 0");

        feed_clean(&mut stats, &mut seq, &mut t, GAP_FRESH_SECS as u64);
        assert_eq!(
            stats.max_gap_age_secs(),
            GAP_FRESH_SECS,
            "a one-off gap riding its flat-top must report a climbing age",
        );

        // --- Recurring: age keeps resetting, because each occurrence writes a
        // fresh young bucket. Same level, opposite cause. ---
        let mut stats = JitterStats::new();
        let t0 = Instant::now();
        stats.observe(1, t0);
        let mut seq = 1u64;
        let mut t = t0;
        for _ in 0..6 {
            t += Duration::from_millis(300);
            seq += 1;
            stats.observe(seq, t);
            assert_eq!(
                stats.max_gap_age_secs(),
                0,
                "a gap that just recurred must report age 0 however long the level \
                 has been flat",
            );
            feed_clean(&mut stats, &mut seq, &mut t, 2);
        }
    }

    /// v5 root cause 6: with the old per-packet `iat_adjusted` feeding, a DTIM
    /// gap was ONE sample among the ~20 burst packets that followed it, so it
    /// sat at the 98th percentile and was invisible at p95 — the histogram could
    /// only ever learn a gap by starving on it. NetEQ's relative-arrival-delay
    /// feeding makes every packet of the burst carry the gap's lateness, so one
    /// gap becomes a whole spread of large samples.
    #[test]
    fn relative_delay_histogram_sees_dtim_bursts() {
        let mut stats = JitterStats::new();
        let mut t = Instant::now();
        let mut seq = 1u64;
        stats.observe(seq, t);

        // 181ms of silence, then 19 packets 1ms apart: 20 frames of audio
        // delivered in 200ms of wall clock — real-time honest, but batched.
        for _ in 0..75 {
            t += Duration::from_millis(181);
            seq += 1;
            stats.observe(seq, t);
            for _ in 0..19 {
                t += Duration::from_millis(1);
                seq += 1;
                stats.observe(seq, t);
            }
        }

        assert!(
            stats.iat_percentile_target >= 15.0,
            "p95 relative delay must reflect the ~18-frame DTIM gap, got {:.1}",
            stats.iat_percentile_target,
        );
    }

    /// The v12 2.4GHz silence, at its root. `relative_packet_arrival_delay` is a
    /// running sum floored at zero, so sustained lateness carries it past the
    /// histogram's 65-frame span; the old code clamped those samples into the top
    /// bin, which then refreshed on every out-of-range observation.
    #[test]
    fn a_relative_delay_beyond_the_histogram_range_must_not_enter_it() {
        assert_eq!(JitterStats::histogram_bin(0.0), Some(0));
        assert_eq!(JitterStats::histogram_bin(3.7), Some(3));
        assert_eq!(
            JitterStats::histogram_bin((IAT_HISTOGRAM_BINS - 1) as f32),
            Some(IAT_HISTOGRAM_BINS - 1),
            "the last representable delay is still a measurement",
        );
        assert_eq!(
            JitterStats::histogram_bin(IAT_HISTOGRAM_BINS as f32),
            None,
            "one frame past the range is already unrepresentable",
        );
        assert_eq!(
            JitterStats::histogram_bin(70.0),
            None,
            "the field value that pinned the target at 63.0 for ~70s",
        );

        // And the discard must be a true no-op on the histogram, not a bin-0 sample.
        let mut stats = JitterStats::new();
        for _ in 0..200 {
            stats.update_iat_histogram(8.0);
        }
        let learned = stats.iat_percentile_target;
        for _ in 0..200 {
            stats.update_iat_histogram(70.0);
        }
        assert_eq!(
            stats.iat_percentile_target, learned,
            "200 out-of-range samples must leave the distribution exactly as it was",
        );
    }

    /// The reverse-CDF walk is bounded by the bin count, so its maximum output is
    /// `IAT_HISTOGRAM_BINS - 1` — 63.0 at the 65 bins we ship. That number is not
    /// a measurement, it is the walk running out of histogram, and the v12 2.4GHz
    /// capture read exactly 63.0 for 22 consecutive windows while `max_gap` held
    /// flat at 30.3 and `gap_floor` *fell*. Under a load the histogram cannot
    /// represent, the body of the distribution must keep governing.
    #[test]
    fn the_histogram_must_not_pin_at_its_top_bin_under_sustained_lateness() {
        let mut stats = JitterStats::new();
        // A bad 2.4GHz stretch: mostly on-time, a steady minority arriving with an
        // accumulated relative delay past the histogram's whole range.
        for i in 0..3000 {
            if i % 12 == 0 {
                stats.update_iat_histogram(60.0 + (i % 30) as f32);
            } else {
                stats.update_iat_histogram((i % 4) as f32);
            }
        }
        let ceiling = (IAT_HISTOGRAM_BINS - 1) as f32;
        assert!(
            stats.iat_percentile_target < ceiling,
            "the walk must never emit its own bound as a delay measurement, got {:.1}",
            stats.iat_percentile_target,
        );
        assert!(
            stats.iat_percentile_target <= 8.0,
            "the in-range body is a 0-3 frame delay; p95 must reflect that rather \
             than the tail max_gap owns, got {:.1}",
            stats.iat_percentile_target,
        );
    }

    /// The other half of the invariant: a delay the histogram *can* represent
    /// must still age out on its own, with no help from a caller. Clamping was
    /// fatal precisely because the ceiling bin was refreshed by every
    /// out-of-range sample and so could never age out at all.
    ///
    /// The descent is a **step, not a glide**, and that is inherent to a
    /// percentile walk rather than a defect: primed to concentrate ~90% of the
    /// mass in one bin, p95 reads 40.0 flat for 16.7s and then drops to 1.0 in a
    /// single update. The arithmetic is exact — the walk stops earlier only once
    /// that bin falls under the 5% tail, which at the Q15 forgetting factor takes
    /// `ln(0.05/0.9) / ln(0.99827) ≈ 1669` updates. It matches the ~17s this
    /// module already documents for shedding 95% of an episode, and it sits
    /// inside the 24s horizon of the gap window beside it, so neither signal can
    /// be the one that pins the target.
    ///
    /// Field distributions are spread across bins rather than concentrated, so
    /// they descend gradually — the v12 5GHz capture shows `histogram` gliding
    /// 15 → 0. The concentrated prime here is the worst case on purpose.
    #[test]
    fn a_learned_delay_should_age_out_of_the_histogram_on_its_own() {
        let mut stats = JitterStats::new();
        for _ in 0..1000 {
            stats.update_iat_histogram(40.0);
        }
        assert!(
            stats.iat_percentile_target >= 39.0,
            "precondition: an in-range 40-frame delay must be learned, got {:.1}",
            stats.iat_percentile_target,
        );

        // 2000 updates is 20s at 100 packets/s, past the measured 16.7s step and
        // still inside the gap window's 24s horizon.
        for _ in 0..2000 {
            stats.update_iat_histogram(1.0);
        }
        assert!(
            stats.iat_percentile_target <= 4.0,
            "a clean link must shed a learned delay without being reset, got {:.1} \
             after 20s — this is the no-self-latching invariant",
            stats.iat_percentile_target,
        );
    }

    /// Rotation is arrival-driven, not wall-clock-driven: during a long outage no
    /// packets arrive, so the window must NOT quietly rotate the outage away
    /// while we are still living through it.
    #[test]
    fn gap_window_is_clamped_and_survives_a_long_silence() {
        let mut stats = JitterStats::new();
        let t0 = Instant::now();
        stats.observe(1, t0);

        // A 3s outage — well past the clamp. `seq_diff` stays small so this is
        // read as pure delivery delay, not loss.
        stats.observe(2, t0 + Duration::from_secs(3));
        assert!(
            (stats.max_gap_frames() - GAP_CLAMP_FRAMES).abs() < 0.01,
            "gaps beyond the clamp must saturate at {} frames, got {:.1}",
            GAP_CLAMP_FRAMES,
            stats.max_gap_frames(),
        );
    }

    /// A stream restart or config change invalidates everything the window
    /// learned about the old link.
    #[test]
    fn resets_clear_the_gap_window() {
        let mut stats = JitterStats::new();
        let t0 = Instant::now();
        stats.observe(1, t0);
        stats.observe(2, t0 + Duration::from_millis(300));
        assert!(stats.max_gap_frames() > 10.0);

        stats.reset_on_stream_restart();
        assert_eq!(stats.max_gap_frames(), 0.0);
        assert!(stats.delay_history.is_empty());

        stats.observe(10, t0 + Duration::from_secs(1));
        stats.observe(11, t0 + Duration::from_millis(1300));
        assert!(stats.max_gap_frames() > 10.0);
        stats.reset_on_config_change();
        assert_eq!(stats.max_gap_frames(), 0.0);
        assert!(stats.delay_history.is_empty());
    }

    /// v12: `inter_burst_gap` read 1485 frames on average and peaked at 5783.5
    /// (57.8s) on a *USB cable* in the v11 field logs, while `max_gap` on the same
    /// capture averaged 7.8 and peaked at 24. The cause was not the EWMA — it was
    /// the cluster anchor surviving events that invalidated it, so the statistic
    /// reported wall-clock time since the last coalescing event rather than a
    /// batching period.
    mod inter_burst_gap_must_measure_a_batching_period {
        use super::*;

        /// Feed one tight cluster of `n` packets, 1ms apart, starting at `*t`.
        /// Leaves `*t` on the last packet of the cluster.
        fn tight_cluster(stats: &mut JitterStats, seq: &mut u64, t: &mut Instant, n: usize) {
            for i in 0..n {
                if i > 0 {
                    *t += Duration::from_millis(1);
                }
                *seq += 1;
                stats.observe(*seq, *t);
            }
        }

        /// The exact v11 shape, minimised: a burst, a long quiet stretch that
        /// expires it, then a second burst. The second burst must measure its gap
        /// against *nothing* — the first burst is no longer a valid reference —
        /// rather than against a 30-second-old anchor.
        #[test]
        fn a_burst_that_expires_should_not_leave_an_anchor_for_the_next_measurement() {
            let mut stats = JitterStats::new();
            let mut t = Instant::now();
            let mut seq = 0u64;

            // Two clusters 200ms apart establish a real batching period.
            tight_cluster(&mut stats, &mut seq, &mut t, 4);
            t += Duration::from_millis(200);
            tight_cluster(&mut stats, &mut seq, &mut t, 4);
            t += Duration::from_millis(200);
            tight_cluster(&mut stats, &mut seq, &mut t, 4);
            assert!(stats.burst_detected(), "setup must trip the detector");
            let honest = stats.inter_burst_gap_frames();
            assert!(
                (10.0..40.0).contains(&honest),
                "a 200ms DTIM period must read ~20 frames, got {honest:.1}",
            );

            // 30 seconds of clean 10ms delivery. Well past BURST_EXPIRY_SECS.
            feed_clean(&mut stats, &mut seq, &mut t, 30);
            assert!(
                !stats.burst_detected(),
                "{BURST_EXPIRY_SECS}s of clean delivery must expire the burst flag",
            );
            assert_eq!(
                stats.inter_burst_gap_frames(),
                0.0,
                "expiry must clear the gap EWMA",
            );

            // A new burst, 30s after the old one. Pre-v12 the first cluster of
            // this pair measured against the *expired* anchor and reported ~3000
            // frames; now there is no anchor to measure against.
            tight_cluster(&mut stats, &mut seq, &mut t, 4);
            t += Duration::from_millis(200);
            tight_cluster(&mut stats, &mut seq, &mut t, 4);
            assert!(stats.burst_detected(), "the new burst must be detected");
            assert!(
                stats.inter_burst_gap_frames() < 40.0,
                "a resumed burst must not inherit wall-clock time since the last one: \
                 got {:.1} frames",
                stats.inter_burst_gap_frames(),
            );

            // And once it has two of its own clusters, it measures its own period.
            t += Duration::from_millis(200);
            tight_cluster(&mut stats, &mut seq, &mut t, 4);
            let relearned = stats.inter_burst_gap_frames();
            assert!(
                (10.0..40.0).contains(&relearned),
                "the new burst must relearn its own ~20-frame period, got {relearned:.1}",
            );
        }

        /// The expiry used to sit *after* the cluster block, so the packet that
        /// resumed after a silence closed a cluster, wrote `last_burst_time = now`,
        /// and the check then saw a fresh timestamp and stood down. It was
        /// structurally unable to fire on the one packet that needed it.
        ///
        /// That packet legitimately re-arms `burst_detected` under either ordering,
        /// so the flag cannot be the observable. The EWMA can: with the old ordering
        /// the cluster block runs first and blends a sample measured against the
        /// pre-silence anchor, carrying the old period across a 10-second gap. With
        /// expiry first, the anchor and the EWMA are both cleared before any
        /// measurement, so the resumed burst starts from nothing.
        #[test]
        fn burst_expiry_should_fire_on_the_packet_that_resumes_after_a_silence() {
            let mut stats = JitterStats::new();
            let mut t = Instant::now();
            let mut seq = 0u64;

            tight_cluster(&mut stats, &mut seq, &mut t, 4);
            t += Duration::from_millis(200);
            tight_cluster(&mut stats, &mut seq, &mut t, 4);
            t += Duration::from_millis(200);
            tight_cluster(&mut stats, &mut seq, &mut t, 4);
            assert!(stats.burst_detected());
            assert!(
                stats.inter_burst_gap_frames() > 10.0,
                "setup must leave a learned period in the EWMA, got {:.1}",
                stats.inter_burst_gap_frames(),
            );

            // A single packet arriving 10s later, closing the pending cluster. This
            // is the packet the old ordering could not expire on: it both ends a
            // cluster and is 10s past the last burst.
            t += Duration::from_secs(10);
            seq += 1;
            stats.observe(seq, t);

            assert_eq!(
                stats.inter_burst_gap_frames(),
                0.0,
                "the resuming packet must expire the stale period before the cluster \
                 block can measure against it",
            );
        }

        /// The bound. `inter_burst_gap` is neither aged nor decayed, so one absurd
        /// sample would sit in the reported signal until the next expiry. The
        /// delivery-gap window already rejects this magnitude as an outage rather
        /// than a comfort-buffer input; the same rule now applies here.
        #[test]
        fn an_inter_burst_gap_wider_than_the_gap_clamp_should_be_rejected() {
            let mut stats = JitterStats::new();
            let mut t = Instant::now();
            let mut seq = 0u64;

            // A burst, then a second burst 3s later — beyond GAP_CLAMP_FRAMES
            // (1.2s) but inside BURST_EXPIRY_SECS (5s), so the anchor is still
            // live and the clamp is the only thing that can reject the sample.
            tight_cluster(&mut stats, &mut seq, &mut t, 4);
            t += Duration::from_secs(3);
            tight_cluster(&mut stats, &mut seq, &mut t, 4);
            t += Duration::from_millis(200);
            tight_cluster(&mut stats, &mut seq, &mut t, 4);

            assert!(
                stats.burst_detected(),
                "the clamp must reject the sample, not the detection",
            );
            assert!(
                stats.inter_burst_gap_frames() <= GAP_CLAMP_FRAMES,
                "a {:.1}-frame inter-burst gap must be rejected at the {:.0}-frame clamp",
                stats.inter_burst_gap_frames(),
                GAP_CLAMP_FRAMES,
            );
        }

        /// The regression this whole module exists for, stated as the field
        /// measured it: on a link that delivers everything on time, the burst
        /// statistic may not report a period the link never had.
        #[test]
        fn a_quiet_link_must_not_accumulate_an_unbounded_inter_burst_gap() {
            let mut stats = JitterStats::new();
            let mut t = Instant::now();
            let mut seq = 0u64;

            // Alternate: a short burst, then a long quiet stretch. Repeat. This is
            // the ADB pattern — scheduler coalescing during activity, silence
            // between. Pre-v12 each new burst measured against the previous one's
            // anchor and the EWMA walked into the thousands.
            for _ in 0..6 {
                tight_cluster(&mut stats, &mut seq, &mut t, 4);
                t += Duration::from_millis(200);
                tight_cluster(&mut stats, &mut seq, &mut t, 4);
                feed_clean(&mut stats, &mut seq, &mut t, 12);
            }

            assert!(
                stats.inter_burst_gap_frames() <= GAP_CLAMP_FRAMES,
                "the reported inter-burst gap must stay bounded on a link whose worst \
                 delivery gap is {:.1} frames, got {:.1}",
                stats.max_gap_frames(),
                stats.inter_burst_gap_frames(),
            );
        }
    }
}
