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

/// Seconds of history in the delivery-gap window. The buffer must survive the
/// worst gap in this window; anything older is no longer representative of the
/// link and should be forgotten so the target can descend.
const GAP_WINDOW_BUCKETS: usize = 24;
/// Buckets younger than this are honoured at full value: a gap that happened in
/// the last 8 seconds is still "the link right now" and must be fully covered.
const GAP_FRESH_SECS: usize = 8;
/// Per-second decay applied to buckets older than [`GAP_FRESH_SECS`].
/// 0.85^16 ≈ 0.07 by the time a bucket rotates out at 24s, so a one-off spike
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
    /// 65-bin Q30 exponential-forgetting histogram of relative arrival delay.
    /// iat_histogram[i] = probability mass that the delay == i frames (Q30).
    iat_histogram: [i64; 65],
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
            iat_histogram: [0i64; 65],
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

        let iat_packets = (delay_frames.max(0.0) as usize).min(64);

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
    pub fn inter_burst_gap_frames(&self) -> f32 {
        self.inter_burst_gap_frames
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
    /// [`GAP_STALE_DECAY`], so a one-off spike glides down from +8s instead of
    /// falling off a cliff at +24s. A *recurring* gap keeps refreshing young
    /// buckets and is therefore unaffected by the weighting.
    pub fn max_gap_frames(&self) -> f32 {
        let n = GAP_WINDOW_BUCKETS;
        let mut max = 0.0f32;
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
            max = max.max(weighted);
        }
        max
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
                            let gap_ms = cur_start
                                .duration_since(prev_start)
                                .as_secs_f32()
                                * 1000.0;
                            let gap_frames = gap_ms / MILLIS_PER_FRAME as f32;
                            // EWMA with α=0.3 — responsive to DTIM period changes
                            // but stable enough to ignore occasional timing jitter.
                            if self.inter_burst_gap_frames == 0.0 {
                                self.inter_burst_gap_frames = gap_frames;
                            } else {
                                self.inter_burst_gap_frames =
                                    self.inter_burst_gap_frames * 0.7 + gap_frames * 0.3;
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

                // Expire burst_detected after BURST_EXPIRY_SECS of silence.
                if self.burst_detected
                    && let Some(last_burst) = self.last_burst_time
                    && arrival_time
                        .duration_since(last_burst)
                        .as_secs()
                        >= BURST_EXPIRY_SECS
                {
                    self.burst_detected = false;
                    self.inter_burst_gap_frames = 0.0;
                    tracing::debug!("[JitterMgr] Burst detection expired");
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
        true
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
        feed_clean(&mut stats, &mut seq, &mut t, 8);
        assert!(
            stats.max_gap_frames() > 95.0,
            "inside GAP_FRESH_SECS the gap must be honoured at full value, got {:.1}",
            stats.max_gap_frames(),
        );

        feed_clean(&mut stats, &mut seq, &mut t, 8); // age 16
        let at_16 = stats.max_gap_frames();
        assert!(
            (20.0..35.0).contains(&at_16),
            "at 16s a 100-frame one-off must have glided to ~27, got {at_16:.1}",
        );

        feed_clean(&mut stats, &mut seq, &mut t, 6); // age 22
        let at_22 = stats.max_gap_frames();
        assert!(
            at_22 < 11.0,
            "at 22s the one-off must be nearly gone, got {at_22:.1}",
        );

        // --- Recurring: a 20-frame gap every 5s must never be discounted. ---
        let mut stats = JitterStats::new();
        let t0 = Instant::now();
        stats.observe(1, t0);
        let mut seq = 1u64;
        let mut t = t0;
        for cycle in 0..10 {
            t += Duration::from_millis(200);
            seq += 1;
            stats.observe(seq, t);
            feed_clean(&mut stats, &mut seq, &mut t, 5);
            if cycle >= 1 {
                // Measured at the oldest point in the cycle — 5s after the last
                // gap, still inside GAP_FRESH_SECS.
                assert!(
                    stats.max_gap_frames() > 19.5,
                    "a gap recurring every 5s must stay fully covered, got {:.1} (cycle {cycle})",
                    stats.max_gap_frames(),
                );
            }
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
}
