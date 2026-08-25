use super::buffer::JitterBuffer;
use super::consts::{
    ARTIFACT_MASK_RMS, CONCEAL_FADE_FLOOR, MILLIS_PER_FRAME, SILENCE_RMS, ms_to_frames_ceil,
};
use super::decoder::FrameDecoder;
use super::flow::PlaybackFlow;
use super::stats::JitterStats;
use super::target::{Band, TargetBreakdown, TargetController, TargetTerm};
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
use std::time::{Duration, Instant};

/// Default stream reset timeout: 1000ms for clean links (5GHz, Ethernet, ADB).
const MAX_MISSING_DEFAULT: u32 = 1000 / MILLIS_PER_FRAME;
/// Extended stream reset timeout for 2.4GHz: DTIM batching routinely produces
/// 1000ms+ silence gaps that are NOT genuine disconnects.
const MAX_MISSING_2_4GHZ: u32 = 3000 / MILLIS_PER_FRAME;
/// Extended stream reset timeout for unknown Wi-Fi links (may be 2.4GHz).
const MAX_MISSING_UNKNOWN: u32 = 2000 / MILLIS_PER_FRAME;
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

/// Inter-splice interval at a *small* target, in callbacks. 6 callbacks ×
/// 10ms/frame = 60ms, slightly above NetEQ's 50ms (`kMinTimescaleInterval` = 5
/// at 10ms frames). This is the rate ADB has run at for four rounds with no
/// artifact report, so it is the calibration point the scaling extends from
/// rather than a value derived afresh.
const TIMESCALE_INTERVAL_BASE: u32 = 6;

/// Floor on the scaled interval, in callbacks. 2 callbacks = 20ms. Below this
/// the rate limiter stops being one: the NCC gate in `accelerate` is the
/// quality control, and this only decides how often it is consulted.
const MIN_TIMESCALE_INTERVAL: u32 = 2;

/// Inter-splice interval for a given target depth, in callbacks.
///
/// **Shortens as the target grows** — the opposite of what "rate limit" first
/// suggests, and the direction is the whole point of the change. A splice moves
/// the buffer by ~1 frame (measured 1.05-1.08 across all three links), so
/// the number of splices needed to cross the drain band is the *band width*,
/// and the band is `target/4` ([`super::target::buffer_limits`]). A flat
/// interval therefore makes band traversal time proportional to the target:
///
/// | link | target | band width | traversal at 6 callbacks flat |
/// | --- | --- | --- | --- |
/// | ADB | 6 | 1 | 60ms |
/// | 5 GHz | 13 | 3 | 180ms |
/// | 2.4 GHz | 36 | 9 | **540ms** |
///
/// That last row is the field measurement: `declined_cooldown` 320 against 118
/// splices on 2.4 GHz — **73% of that link's drain refusals were the rate
/// limiter, not quality**. The same constant that is correct at a 6-frame
/// target is the dominant veto at 36.
///
/// Holding *traversal* time constant instead of inter-splice time divides the
/// ADB-calibrated base by the splices the band needs. Yields 6 at ADB (bit-
/// identical, which is what keeps the change attributable), 2 at both 5 GHz and
/// 2.4 GHz.
///
/// This is the same absolute-constant-across-scales defect `MIN_BAND` once had.
///
/// The emergency (fast-accelerate) tier bypasses the cooldown entirely
/// regardless, matching NetEQ's `kFastAccelerate`.
fn timescale_interval(target: u32) -> u32 {
    let band_width = (target / 4).max(1);
    (TIMESCALE_INTERVAL_BASE / band_width).max(MIN_TIMESCALE_INTERVAL)
}
/// Cooldown applied specifically after a preemptive expand: 20 callbacks =
/// 200ms. Expand inserts exactly one pitch period per call, so at the shared
/// 60ms cooldown a sustained below-target stretch produced a ~17Hz train of OLA
/// splices — the "fast clicking on every buffer increase" from the field test.
/// Expand is now a last-ditch underrun defence rather than a growth mechanism,
/// and one splice per 200ms is the right rate for that job.
const MIN_EXPAND_INTERVAL: u32 = 20;
/// Cooldown after a free-silence growth insert: 3 callbacks. Each insert adds a
/// whole 10ms frame of latency, so firing on *every* below-target callback is a
/// 2× time-stretch — correct in direction but far too steep, and it accrues
/// latency the descent then has to give back. One frame per 3 callbacks is a
/// ~33% stretch: still faster than any real drain, gentle enough that the
/// latency bought is proportional to the deficit.
const MIN_SILENCE_GROW_INTERVAL: u32 = 3;
/// Floor on the overshoot above `high_limit` that promotes a drain to the
/// emergency tier. 5 frames = 50ms. See [`emergency_threshold`].
const EMERGENCY_MIN_MARGIN: u32 = 50 / MILLIS_PER_FRAME;

/// Filtered buffer level at or above which a drain is promoted to the emergency
/// (fast) tier: cooldown bypassed, correlation threshold dropped 0.9 → 0.5.
///
/// `high_limit + max(EMERGENCY_MIN_MARGIN, high_limit / 2)` — proportional, with
/// a floor.
///
/// Two prior forms, both measured and both wrong in the same way: an absolute
/// constant applied across targets that span 5 to 50 frames.
///
///  * `4 * high_limit` (~2 seconds of audio) never fired. Removed for that.
///  * `high_limit + 15` (a flat 150ms) replaced it and did not fire either: the
///    threshold landed at 22.7 frames on ADB, 24.3 on 5GHz and 49.8 on 2.4GHz,
///    against filtered levels that parked at 9.12 / 9.86 / 18.79. Measured firing
///    rate **0.0% / 0.2% / 0.0% of windows**. Peak occupancy 310 / 280 / 400ms —
///    the "buffer jumps to 250ms++" report is this threshold being the buffer's
///    effective ceiling rather than a tier it can reach.
///
/// A flat margin is a *smaller* fraction of a large target than of a small one,
/// which is backwards: a 150ms overshoot on ADB's 50ms target is catastrophic and
/// on 2.4GHz's 350ms target is ordinary. Scaling with `high_limit` fixes the
/// direction — the emergency tier now sits at 1.5× the drain limit everywhere.
///
/// Upstream's is proportional too, `filtered >= high_limit << 2`
/// (`decision_logic.cc:283-296`), but that is exactly the `4 * high_limit` this
/// module already removed for never firing; NetEQ can afford it because its
/// normal tier actually drains, and ours once did not. Half is the value
/// that puts the tier inside the measured occupancy range on every link:
/// ADB ~11 frames, 5GHz ~14, 2.4GHz ~52.
///
/// The floor matters at small targets, where `high_limit / 2` would collapse into
/// the normal tier's own dead-band (`max(target/4, 2 frames)`): at `high = 3` the
/// proportional part is 1 frame, so ordinary oscillation would promote to the
/// riskier 0.5-correlation splice. 50ms is 2.5 dead-bands at the floor — enough
/// that the normal tier gets a real attempt first.
fn emergency_threshold(high_limit: u32) -> u32 {
    high_limit + EMERGENCY_MIN_MARGIN.max(high_limit / 2)
}
/// Consecutive starved callbacks (50ms of PLC) after which playback re-enters
/// prebuffering instead of resuming on the first frame that shows up.
///
/// Resuming on `has_next` — one single frame — is what turned one delivery gap
/// into a machine-gun of stutters in the field (2.4GHz Router A: six starvation
/// events in six seconds). The mechanism: packets arrive at real time and the DAC
/// consumes at real time, so occupancy cannot grow while the playhead is running.
/// Neither growth path can bank depth here either — silence-grow needs
/// `rms < SILENCE_RMS`, and expand is an underrun defence that only fires at
/// `occupied <= 1` and inserts one pitch period per 200ms — which leaves starving
/// again as the *only* way to bank depth. Pausing the playhead until the buffer
/// reaches `resume_threshold_pct * target` banks it in one go: one clean gap
/// instead of six audible ones, and the floor/stat bookkeeping runs once.
///
/// On DTIM-batched links the next burst refills that threshold instantly, so the
/// pause costs no more wall-clock silence than the gap already did.
const REBUFFER_AFTER: u32 = 5;

/// Callbacks between depth-authority observability lines — one per second at
/// 10ms frames. See [`JitterBufferManager::log_depth_authority`].
const LOG_INTERVAL_CALLBACKS: u32 = 1000 / MILLIS_PER_FRAME;

/// Fraction of the nominal packet rate below which delivery is reported as
/// under-delivery. See [`JitterBufferManager::log_depth_authority`] for why the
/// comparison is against wall-clock rather than against `frames_played`.
///
/// Sited on field captures, where `arrivals / expected` is cleanly bimodal.
/// ADB (205 windows) and 5GHz (257) never read below **0.90**. 2.4GHz splits
/// into a collapse population of **24 windows at 0.33-0.57** and a healthy
/// remainder whose lowest reading is **0.74**. The bucket `[0.60, 0.70)` is
/// empty on all three links, so this threshold separates the two populations
/// instead of cutting through either.
const UNDER_DELIVERY_RATIO: f32 = 0.7;

/// Per-window tally of what the timescale layer actually did, emitted on the
/// 1Hz depth line and cleared with it.
///
/// The splices themselves are logged at `trace!`, which the mobile crate's
/// `LevelFilter::Info` discards — three field captures across ADB, 2.4GHz and
/// 5GHz contained **zero** accelerate/expand/drain lines between them, so the
/// subsystem that performs every edit to the audio had never been observed in
/// the field. Counting is an increment on the hot path and costs one `info!`
/// field per second; a per-splice log at `info!` would be a logging burst in the
/// audio callback, which is the thing this module is not allowed to do.
///
/// `declined_*` are the two ways [`JitterBufferManager::process_next_frame`]
/// reaches the drain branch and returns without draining. Distinguishing them
/// is the whole point: `cooldown` means the rate limiter is the binding
/// constraint, `rms_mask` means program material is, and removing that mask was
/// the worst regression in this module's history — so the question "is the mask
/// blocking the drain you want?" has to be answerable from a log rather than
/// from argument.
///
/// The field answered it: `declined_rms_mask` took **93-96% of every
/// drain attempt on all three links** (36 455 attempts, 243 splices, over 881s),
/// while `declined_recovery` was zero everywhere and `declined_cooldown` under
/// 4%. `rms_sum` / `rms_count` / `rms_max` were added next so the *threshold*
/// can be sited from measurement rather than re-argued: the counts say the gate
/// declines, the RMS says by how much.
#[derive(Default, Clone, Copy, PartialEq, Debug)]
struct TimescaleTally {
    /// WSOLA accelerate splices that returned a shortened window.
    accelerated: u32,
    /// WSOLA expand splices (imminent-underrun defence).
    expanded: u32,
    /// Whole packets shed by the silence fast-forward shortcut.
    shed: u32,
    /// Frames of silence appended by the free-growth path.
    grown: u32,
    /// Frames removed by accelerate, summed (fractional — a splice removes one
    /// pitch period, not one frame).
    removed_frames: f32,
    /// Frames inserted by expand, summed.
    inserted_frames: f32,
    /// WSOLA accelerate splices that fired on the fast (emergency) tier.
    /// The fast tier bypasses the cooldown and drops the correlation threshold
    /// 0.9 → 0.5, so it is the most artifact-prone edit path in the module.
    /// It shipped with **no counter at all** — three field captures show zero
    /// fast-tier lines because the splice log is `trace!` and logcat discards
    /// it. This count closes that gap.
    fast_accelerated: u32,
    /// Preemptive expand splices — the `filtered < low_limit` growth path.
    preemptive: u32,
    /// Preemptive expand was attempted (`filtered < low_limit`) but declined
    /// by its own NCC gate inside `expand()`.
    declined_preemptive_ncc: u32,
    /// The *imminent-underrun* tier of expand was attempted (`occupied <= 1`) and
    /// declined by the same NCC gate.
    ///
    /// Split out because [`TimescaleTally::declined_preemptive_ncc`] is
    /// incremented only `if !imminent_underrun` — so the declines at the very edge
    /// of starvation, the ones with the least margin left to recover from, were
    /// the single population the census could not see. A run where expand is armed
    /// constantly at one frame and refused every time looks, in every other field
    /// on this line, exactly like a run where the tier never armed.
    declined_underrun_ncc: u32,
    /// Over `high_limit`, but the timescale cooldown had not expired.
    declined_cooldown: u32,
    /// Over `high_limit` and off cooldown, but the frame was too loud for the
    /// splice to be masked (`rms >= ARTIFACT_MASK_RMS`).
    ///
    /// **Must now read 0 on every link.** The masking gate stopped being a
    /// precondition for the drain, so nothing increments this any more — it is
    /// kept, and kept printing, as the tripwire that would say the gate came
    /// back by some path nobody intended. It read 93-96% of every attempt in the
    /// field, which is why it is worth a permanent zero.
    declined_rms_mask: u32,
    /// Over `high_limit`, but inside the post-starvation recovery guard.
    declined_recovery: u32,
    /// `accelerate` was attempted and declined by its own NCC gate.
    declined_ncc: u32,
    /// Splices that landed on content the old masking gate would have refused
    /// (`rms >= ARTIFACT_MASK_RMS`).
    ///
    /// The count that makes the demotion's central risk falsifiable in the field: it is
    /// exactly the population of edits that did not exist before, so a reported
    /// warble on sustained tones can be weighed against how many loud splices
    /// produced it — and silence can be weighed against the same number.
    loud_splices: u32,
    /// Sum of the per-frame content RMS the masking gate read, over the window.
    ///
    /// The RMS is already computed on the hot path for the gate check itself, so
    /// this is one add and one compare — but its *value* has never been logged.
    /// Every claim about where program material sits relative to
    /// [`ARTIFACT_MASK_RMS`] has therefore been inference from the decline
    /// counts, the same position `max_gap_age` was in before it became a logged
    /// field. Summed rather than latched so it reports a window average and
    /// falls on its own the moment the content does — a max-only reading would
    /// latch its own history, which this module does not permit.
    rms_sum: f32,
    /// Frames contributing to [`TimescaleTally::rms_sum`]. Only frames that
    /// reached the gate are counted, so the average describes the content the
    /// gate actually judged rather than the whole window.
    rms_count: u32,
    /// Loudest frame the gate saw this window. Bounded by the window, so it
    /// cannot carry a peak forward — reported alongside the average because a
    /// quiet mean with loud peaks and a uniformly loud window decline at the
    /// same rate but call for different thresholds.
    rms_max: f32,
    /// Sum of the content RMS over the frames that actually *spliced* this window.
    ///
    /// [`TimescaleTally::rms_sum`] above describes every frame the gate judged;
    /// this describes the subset it admitted, which is the population
    /// [`TimescaleTally::loud_splices`] classifies and the only one that can
    /// produce an artifact. The two together say whether the splices are landing
    /// on the loud part of the window or the quiet part — the printed line used to
    /// answer that with `mask_rms=`, which was the *constant* `ARTIFACT_MASK_RMS`
    /// and has read 0.08 on every window of every capture since it was demoted.
    /// Summed and window-scoped, so it falls the moment the content does.
    splice_rms_sum: f32,
    /// Splices contributing to [`TimescaleTally::splice_rms_sum`].
    splice_rms_count: u32,
    /// Worst terminal seam produced by a splice this window, in units of the
    /// incoming signal's own steepest step across the crossfade region — see
    /// [`super::timescale::TimeScaler::take_splice_step`].
    ///
    /// The fade fix — a full Hann *bell* (0 → 1 → 0) replaced by a monotonic
    /// ramp — is the only change in this module's history that shipped
    /// **unconfirmed by ear**, and it cannot be confirmed by a unit test either: at
    /// an exact pitch multiple the two shapes produce byte-identical output, which
    /// is how the bell survived four rounds of green tests. This is the field
    /// discriminator. The monotonic ramp bounds it at **1.00 by construction**; the
    /// bell measured **0.37-6.6** on the same material. A capture that reads ≤1.00
    /// answers "did the ramp help?" with no listening at all, and a reading above
    /// 1.00 says a non-monotonic fade came back.
    ///
    /// Window-bounded exactly as [`TimescaleTally::rms_max`] is: it is a max over
    /// the window's splices and resets with the window, so it cannot latch its own
    /// history.
    splice_step_max: f32,
    /// Longest run of consecutive concealed callbacks seen this window — the peak
    /// of [`super::flow::PlaybackFlow::conceal_run`].
    ///
    /// Upstream bounds concealment quality by `consecutive_expands_`
    /// (`expand.cc:150-312`): a mute schedule steps at 3 and 7 and hands over to
    /// background noise at 0. Ours is bounded only by `MIN_EXPAND_INTERVAL` and
    /// `REBUFFER_AFTER`, and for a long time **nothing counted the run at all**,
    /// so that bound was inferred rather than measured.
    ///
    /// A max over the window, cleared with the rest of the tally by
    /// `std::mem::take`, so it cannot carry a peak past the window that produced
    /// it — the same shape as [`TimescaleTally::rms_max`].
    conceal_run_max: u32,
    /// Concealed frames this window that repeated the last played frame's pitch
    /// period instead of asking the codec to extrapolate
    /// ([`super::timescale::TimeScaler::conceal_frame`]).
    ///
    /// Reads 0 on an Opus stream by construction — `plc_ready()` is true there, so
    /// the codec path is taken. On an uncompressed or silence-heavy stream it should
    /// account for **all** of `conceal_run`'s frames; a run without conceals beside
    /// it means the history was missing and the concealment was digital silence.
    pitch_conceals: u32,
    /// Worst entry seam produced by a pitch concealment this window, in the same
    /// units as [`TimescaleTally::splice_step_max`] — see
    /// [`super::timescale::TimeScaler::take_conceal_step`].
    ///
    /// Separate from `splice_step_max` on purpose. That counter's whole value is its
    /// **≤1.00 ceiling**: a monotonic fade cannot exceed it, so any reading above
    /// 1.00 says a non-monotonic fade came back. A concealment entry is a *hard*
    /// join — no crossfade covers it, because there is no incoming signal to fade
    /// from — so it legitimately reads above 1.00 whenever the material's level
    /// moved across the repeated period. Merged, one field could no longer tell a
    /// fade regression from concealment landing on a decaying note.
    ///
    /// Window-scoped max, cleared with the rest of the tally.
    conceal_step_max: f32,
    /// Concealed frames emitted at exactly [`super::consts::CONCEAL_FADE_FLOOR`] —
    /// the tail of a run long enough to have decayed all the way down.
    ///
    /// The fade floor accepts one risk, and this is the counter that prices it.
    /// A long run repeats a single pitch period at the floor gain for its whole
    /// tail, where upstream rotates three lags (`expand.cc:844-853`) so consecutive
    /// expansions are never identical; ours re-stages history from its own output,
    /// so the pitch search reconverges on the same period. The risk is therefore a
    /// *long* run, not a frequent one — and [`TimescaleTally::conceal_run_max`]
    /// cannot separate those, because it reports the window's longest run without
    /// saying how much audio that run actually emitted at the floor. A window with
    /// one 40-frame run and a window with four 14-frame ones read identically
    /// there and differ 26:4 here.
    ///
    /// A count over the window, cleared with the rest of the tally by
    /// `std::mem::take`, so it cannot carry its own history forward.
    floor_frames: u32,
    /// Packets that arrived behind the playhead and were rejected
    /// ([`super::buffer::InsertResult::Stale`]).
    ///
    /// `stats.observe` counts the arrival *before* `buffer.insert` runs, so a
    /// rejected packet is already inside `arrivals` and can never appear in
    /// `played`. For a long time nothing counted it, which is why 6.2% of every
    /// arrival was unattributable: measured `arrivals - played` = 1764 frames
    /// (7.6%, 17.6s of audio) across one uncompressed capture, of which only
    /// 340 were explained by the logged flush/shed paths.
    stale_rejects: u32,
    /// Summed `next_play_seq - seq_num` over this window's stale rejects.
    ///
    /// The count alone cannot separate the two causes. A lag of 1-2 frames is an
    /// ordinary reorder the buffer is built to absorb; a lag of 20+ means the
    /// playhead skipped over packets that were still in flight and then threw
    /// them away on arrival — [`TimescaleTally::playhead_skips`] manufacturing
    /// its own stale rejects. Summed for a window mean, never latched.
    stale_lag_sum: u64,
    /// Worst `next_play_seq - seq_num` this window. Window-bounded exactly as
    /// [`TimescaleTally::rms_max`] is, so it falls the moment the condition
    /// does — a running maximum would latch its own history.
    stale_lag_max: u32,
    /// Times the playhead was advanced across a hole rather than waiting for the
    /// missing packet (`advance_one` or `fast_forward`).
    playhead_skips: u32,
    /// Frames the playhead jumped over, summed. `advance_one` contributes 1;
    /// `fast_forward` contributes its full `diff`.
    ///
    /// Armed by [`REORDER_TOLERANCE_2_4GHZ`] — 6 frames, 60ms — against a link
    /// whose *median* delivery gap measured 21.6 frames (216ms) in the field.
    /// Whether that tolerance is too tight to be honest is open, and this counter
    /// is what decides it.
    skipped_frames: u32,
    /// Frames discarded by [`JitterBufferManager::flush_with_crossfade`], from
    /// all three callers (startup flush, rebuffer clamp, no-buffer drain).
    ///
    /// Each caller already logs its own line, but reconstructing the total from
    /// three different log shapes is inference; this is the measurement. With
    /// the counters above it closes the ledger: `arrivals - played - staged` must
    /// equal `stale_rejects + skipped_frames + flush_discards + shed`, and a
    /// residual means a fifth sink exists.
    flush_discards: u32,
    /// Packets popped by the accelerate path to *extend* its staging window.
    ///
    /// The last term of the ledger above, and the reason it did not close
    /// before. This pop takes a second packet out of the buffer in the same
    /// callback that already played one, and `frames_played` counts callbacks
    /// rather than packets — so the frame was emitted (spliced, or verbatim via
    /// `StagedWindow::emit`) while `arrivals - played` still counted it as lost.
    /// Measured: `unplayed` read as 6.8% packet loss on a 2.4GHz uncompressed
    /// capture where true loss was 0%, and the ledger's own doc had to end with
    /// "plus the fractional accelerate pops" because nothing counted them.
    /// `unplayed` on the depth line now subtracts this, and prints it beside the
    /// result so the correction stays visible.
    staged_pops: u32,
}

/// Everything one callback resolved before it decided what to play.
///
/// Bundled rather than passed as five parameters because the depth the
/// controller settled on, the raw measurement behind it and the floor under
/// both are read by four separate phases of [`JitterBufferManager::process_next_frame`],
/// and a phase that saw a different `now` than its neighbours could disagree
/// with them about the same instant.
#[derive(Clone, Copy)]
struct FrameContext {
    /// One clock read for the whole callback.
    now: Instant,
    /// The controller's ramped, hysteresis-quantized depth for this callback.
    target: u32,
    /// The live comfort-capped answer before ramping, from
    /// [`TargetController::target_breakdown`].
    raw_target: u32,
    /// Static config floor under both.
    min_depth: u32,
    /// `static_target_ms == Some(0)` — the user asked for no buffering at all.
    is_no_buffer: bool,
    /// ADB/USB transport, which needs its own target cap and skips the
    /// starvation floor bump.
    tcp_mode: bool,
}

/// Everything the 1 Hz depth line accumulates, plus the anchor it measures its
/// own span from.
///
/// Grouped because the rotation has to be all-or-nothing: anything that clears
/// some of these and not the rest reads what survived the previous flush, which
/// is a standing trap for any measurement spanning more than one window.
/// [`Self::rotate`] is the only place any of them is reset.
#[derive(Default)]
struct LogWindow {
    /// Callbacks since the last depth-authority log line.
    frame_count: u32,
    /// Wall-clock instant the current depth-authority window opened. `None`
    /// until the first line is emitted. Lets the log report what the window
    /// actually spanned instead of assuming `LOG_INTERVAL_CALLBACKS * 10ms` — on
    /// 2.4GHz the callback interval stretched to 3161ms during a measured
    /// collapse, which silently rescaled every per-window rate printed.
    started_at: Option<Instant>,
    /// Frames actually emitted from real packets since the last log line. Paired
    /// with the arrival count to expose a delivery rate below the playback rate.
    frames_played: u32,
    /// What the timescale layer did during the current log window.
    tally: TimescaleTally,
}

impl LogWindow {
    /// Close this window and open the next: return what it accumulated — frames
    /// played, the timescale tally, and how long it actually spanned (`None` for
    /// the first window, which has no prior anchor).
    ///
    /// `started_at` rotates to `now` rather than to its default, because it
    /// anchors the *next* window; clearing it would drop the span onto the
    /// nominal-callback fallback on every window from here on. That is why this
    /// is a method and not a `mem::take` of the whole struct.
    fn rotate(&mut self, now: Instant) -> (u32, TimescaleTally, Option<Duration>) {
        let played = std::mem::take(&mut self.frames_played);
        let tally = std::mem::take(&mut self.tally);
        self.frame_count = 0;
        let spanned = self
            .started_at
            .replace(now)
            .map(|started| now.duration_since(started));
        (played, tally, spanned)
    }
}

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
    /// Stamping point for true NIC->DAC millisecond latency. Shared with player backend.
    latency_metric: Arc<AtomicU32>,
    /// Rolling jitter estimate (`stats.ema_jitter`) in whole milliseconds, mirrored
    /// out to the player backend the same way as `latency_metric`. Distinct signal:
    /// `latency_metric` is per-frame buffer dwell time, this is network arrival jitter.
    jitter_metric: Arc<AtomicU32>,
    config: JitterConfig,
    config_ref: Arc<RwLock<JitterConfig>>,
    is_tcp_mode: Arc<AtomicBool>,
    /// The detected network link for this session. Constant for the session's
    /// lifetime (cached at connect, so passed by value rather than shared),
    /// it lets the runtime tune link-specific policy — currently the reorder
    /// tolerance — instead of collapsing everything to the connect-time
    /// `JitterConfig` snapshot plus the coarse `is_tcp_mode` bool.
    network_link: NetworkLink,
    /// WSOLA time-scaler (crossfade ramp + scratch buffer for expand/accelerate/splice).
    timescale: TimeScaler,
    /// Countdown to reduce config lock polling: only check every 100 frames (~500ms).
    config_check_countdown: u32,
    /// Test-only injected clock. When `Some`, [`Self::now`] returns it instead of
    /// the real wall clock, so a test can drive the wall-clock-gated logic (the
    /// floor-relax timers, recovery window, latency stamps) along the same
    /// simulated timeline its packet arrivals use. `None`/absent in production —
    /// gated to `cfg(test)` so the hot path compiles to a bare `Instant::now()`.
    #[cfg(test)]
    test_clock: Option<Instant>,

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
    /// The 1 Hz depth-line accumulators and the anchor for their span.
    log_window: LogWindow,
}

impl JitterBufferManager {
    pub fn new(
        decoder: Decoder,
        latency_metric: Arc<AtomicU32>,
        jitter_metric: Arc<AtomicU32>,
        config_ref: Arc<RwLock<JitterConfig>>,
        is_tcp_mode: Arc<AtomicBool>,
        network_link: NetworkLink,
    ) -> Self {
        let initial_config = config_ref.read().unwrap().clone();
        let stats = JitterStats::new();

        Self {
            decoder: FrameDecoder::new(decoder),
            buffer: JitterBuffer::new(),
            playback_buf: VecDeque::with_capacity(OPUS_FRAME_SAMPLES * 100),
            latency_metric,
            jitter_metric,
            config: initial_config,
            config_ref,
            is_tcp_mode,
            network_link,
            timescale: TimeScaler::new(),
            config_check_countdown: 0,
            #[cfg(test)]
            test_clock: None,
            control: TargetController::new(),
            flow: PlaybackFlow::new(),
            stats,
            timescale_cooldown: 0,
            pending_gap_fadein: false,
            startup_flush_pending: true,
            log_window: LogWindow::default(),
        }
    }

    /// The current instant for this callback. In production this is the real
    /// wall clock, read once per callback. In tests it is an injected simulated
    /// instant when one is set, so wall-clock-gated logic advances on the same
    /// timeline the test's packet arrivals do.
    #[cfg(not(test))]
    #[inline]
    fn now(&self) -> Instant {
        Instant::now()
    }

    #[cfg(test)]
    fn now(&self) -> Instant {
        self.test_clock.unwrap_or_else(Instant::now)
    }

    /// Test-only: advance (or set) the injected clock. Panics are impossible —
    /// `None` simply starts from the real clock on first use.
    #[cfg(test)]
    fn set_test_clock(&mut self, now: Instant) {
        self.test_clock = Some(now);
    }

    /// Get the minimum buffer depth in frames.
    fn min_depth_frames(&self) -> u32 {
        ms_to_frames_ceil(self.config.min_depth_ms)
    }

    /// Packets the streamer should have produced over `window_ms` of wall clock.
    ///
    /// The streamer emits one packet per frame at real time, so the nominal rate
    /// is fixed by the frame size and independent of anything the player does
    /// — which is exactly the property [`Self::is_under_delivering`] needs and
    /// `frames_played` lacks.
    fn expected_packets(window_ms: u32) -> u32 {
        window_ms / MILLIS_PER_FRAME
    }

    /// Whether arrivals over a window fell far enough below the nominal packet
    /// rate to call the link unable to carry the stream.
    ///
    /// Deliberately *not* a comparison against playback. The player's own
    /// callback rate is a dependent variable — when delivery collapses the
    /// callbacks stretch with it, so any ratio between two player-side
    /// counters stays near 1.0 through the collapse it is supposed to detect.
    /// Wall clock is the one term in this decision the failure cannot move.
    fn is_under_delivering(arrivals: u32, expected: u32) -> bool {
        (arrivals as f32) < expected as f32 * UNDER_DELIVERY_RATIO
    }

    /// Fold one *successful* splice into the window's quality census: the content
    /// energy it landed on, and the terminal seam it produced.
    ///
    /// Called from both actuators so the two readings always describe the same
    /// population — every splice, and only splices. `avg_rms` beside it describes
    /// every frame the gate *judged*, which is a different and much larger set.
    ///
    /// Both are window-scoped: the sum is divided by its own count when the line
    /// is emitted, the step is a max over the window's splices, and
    /// `std::mem::take` clears them with the rest of the tally. Neither can carry
    /// a value past the window that produced it.
    fn note_splice_quality(&mut self, rms: f32) {
        self.log_window.tally.splice_rms_sum += rms;
        self.log_window.tally.splice_rms_count += 1;
        if let Some(step) = self.timescale.take_splice_step() {
            self.log_window.tally.splice_step_max = self.log_window.tally.splice_step_max.max(step);
        }
    }

    /// Link-aware stream reset timeout in frames. On 2.4GHz / Unknown, DTIM
    /// batching routinely produces 1000ms+ silence gaps that are NOT genuine
    /// disconnects. A longer timeout prevents false resets that wipe learned state.
    fn max_missing_for(link: NetworkLink) -> u32 {
        match link {
            NetworkLink::Wifi2_4Ghz => MAX_MISSING_2_4GHZ,
            NetworkLink::WifiUnknown | NetworkLink::Unknown => MAX_MISSING_UNKNOWN,
            _ => MAX_MISSING_DEFAULT,
        }
    }

    /// Pure computation of the target buffer depth from observed jitter statistics,
    /// retaining every input term so the once-per-second observability line can name
    /// the winner. Delegates to the [`TargetController`] actor.
    fn target_breakdown(&self, tcp_cap_override: Option<f32>) -> TargetBreakdown {
        self.control
            .target_breakdown(&self.config, &self.stats, tcp_cap_override)
    }

    /// Drain all pending raw packets from the SPSC channel into the jitter buffer.
    /// Updates Dual-EMA jitter statistics from observed inter-arrival times.
    pub fn ingest_packets(&mut self, consumer: &mut HeapCons<RawPacket>) {
        while let Some(pkt) = consumer.try_pop() {
            // Update jitter statistics from this arrival. Returns false to drop the
            // packet entirely (clock ran backwards vs. the last forward arrival).
            if !self.stats.observe(pkt.seq_num, pkt.arrival_time) {
                continue;
            }
            // Captured before `insert` takes ownership; needed to measure how far
            // behind the playhead a stale rejection landed.
            let seq_num = pkt.seq_num;

            use super::buffer::InsertResult;
            match self.buffer.insert(pkt) {
                InsertResult::StreamRestarted => self.decoder.resync(),
                // Arrived behind the playhead. `stats.observe` already counted
                // it, so this frame is inside `arrivals` and can never reach
                // `played` — the ledger only balances if it is counted here.
                InsertResult::Stale => {
                    let lag = self.buffer.next_play_seq().saturating_sub(seq_num) as u32;
                    self.log_window.tally.stale_rejects += 1;
                    self.log_window.tally.stale_lag_sum += lag as u64;
                    self.log_window.tally.stale_lag_max =
                        self.log_window.tally.stale_lag_max.max(lag);
                }
                InsertResult::Accepted => {}
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
        // One clock read for the whole callback: the recovery window, the
        // latency stamp and the starvation bookkeeping all use this instant, so
        // they cannot disagree and the hot path pays for `Instant::now()` once.
        // Tests inject a simulated timeline via `test_clock` so the floor-relax
        // timers advance on the same `Instant`s the packet arrivals do, rather
        // than on a real clock a test cannot move.
        let now = self.now();
        self.flow.tick_recovery(now);

        self.reload_config_if_changed();
        let ctx = self.resolve_target(now);
        self.apply_no_buffer_flush(&ctx);
        if self.serve_prebuffering(&ctx) {
            return;
        }
        self.apply_startup_flush(&ctx);
        self.apply_static_target_flush(&ctx);
        self.try_fast_forward_over_gap(&ctx);
        if self.buffer.has_next() {
            self.play_next_frame(&ctx);
            return;
        }
        self.handle_missing_frame(&ctx);
    }

    /// Poll the shared config and, if it changed, restart convergence.
    ///
    /// The flush is deferred out of the `try_read` block because the guard
    /// borrows `self.config_ref` and `flush_with_crossfade` needs all of `self`.
    fn reload_config_if_changed(&mut self) {
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
                self.config = new_config;
            }
        }
        if let Some(flush_target) = pending_flush {
            self.flush_with_crossfade(flush_target);
        }
    }

    /// Resolve this callback's depth authority and emit the 1Hz depth line.
    fn resolve_target(&mut self, now: Instant) -> FrameContext {
        let min_depth = self.min_depth_frames();
        let tcp_mode = self.is_tcp_mode.load(Ordering::Relaxed);
        // USB/ADB multiplexing proxy naturally introduces transient OS locks and micro-jitter.
        let breakdown = if tcp_mode {
            // Cap at 12 frames (60ms) to prevent overbuffering on USB.
            // If the user selected a low-latency preset like Wired, this also overrides their
            // native comfort cap (e.g. 4 frames) so ADB can safely absorb massive USB-transit batching.
            let dynamic = self.target_breakdown(Some(12.0));
            // Allow user to overwrite natively if they chose Static
            if let Some(static_ms) = self.config.static_target_ms {
                let raw = ms_to_frames_ceil(static_ms).max(self.min_depth_frames());
                TargetBreakdown {
                    pre_clamp: raw as f32,
                    winning: TargetTerm::Static,
                    raw,
                    ..dynamic
                }
            } else {
                dynamic
            }
        } else {
            self.target_breakdown(None)
        };
        let raw_target = breakdown.raw;

        let is_no_buffer = self.config.static_target_ms == Some(0);

        // --- Hysteresis + quantization + rate-limited ramping ---
        // Delegated to the target controller (handles static-mode bypass, dwell,
        // ramp, and downward probing internally).
        let target = self
            .control
            .advance(&self.config, &self.stats, raw_target, min_depth, now);

        self.log_depth_authority(&breakdown, target, now);

        FrameContext {
            now,
            target,
            raw_target,
            min_depth,
            is_no_buffer,
            tcp_mode,
        }
    }

    /// No-buffer mode keeps its own aggressive emergency flush (latency is
    /// the overriding concern there). In normal mode we no longer flush on a
    /// multiple of target — the NetEQ decision band below drains via WSOLA
    /// instead, with the `emergency_threshold` fast tier acting as the
    /// emergency drain. `flush_with_crossfade` is thus reserved for config
    /// changes and no-buffer mode.
    fn apply_no_buffer_flush(&mut self, ctx: &FrameContext) {
        if ctx.is_no_buffer {
            // Latency is paramount here, so drain on *instantaneous* occupancy
            // rather than the lagging filtered level, straight down to a single
            // frame. The NetEQ decision band below (with its 20ms WINDOW_20MS
            // floor) is deliberately bypassed for no-buffer — that window would
            // hold ~20ms the user explicitly asked not to buffer.
            if self.buffer.occupied_count() > ctx.target + 1 {
                self.flush_with_crossfade(ctx.target + 1);
            }
        }
    }

    /// Hold the playhead until the buffer refills, and clamp the burst that
    /// releases it.
    ///
    /// Returns `true` when this callback is finished — the playhead is still
    /// held and concealment has been emitted. A *release* falls through to the
    /// rest of the callback and returns `false`, as does not prebuffering at all.
    fn serve_prebuffering(&mut self, ctx: &FrameContext) -> bool {
        if !self.flow.is_prebuffering {
            return false;
        }
        // The resume threshold is a pure function of the target. A
        // `max_gap`-derived floor was once added here and it failed in the
        // field: the gap that *caused* the rebuffer is folded into the window
        // by the packet that ends it, so the floor read at prebuffer exit is
        // always ≥ that gap — 5GHz jumped 3 → 21 frames on a control link.
        // `max_gap` is logged below as an observation only; do not wire it
        // back into the depth.
        //
        // **An absolute 150ms ceiling used to sit here and was removed**
        // (`REBUFFER_HOLD_CAP_MS`). It was written to bound the hole width
        // independently of the depth, on the reasoning that a deeper
        // threshold means a longer hold. On a DTIM-batched link that premise
        // is false: `ingest_packets` drains the whole ring before this test
        // runs, so the hold ends when a *burst* lands, not when the threshold
        // is crossed — measured overshoot at the crossing callback averaged
        // **+9.3 frames** (uncompressed) / **+12.9** (128kbps), and **16 of 21
        // resumes already held ≥ `0.5 * target` at the moment they exited**.
        // Removing the cap cost **370ms of extra hold across a 4m42s capture**
        // against **4835ms spent starved**. See the doc on `REBUFFER_AFTER`
        // above, which had already recorded the same fact from the other side.
        //
        // What the cap did instead was resume at 15 frames (150ms) on a link
        // whose *median* delivery gap measured **21.6 frames (216ms)**, p90
        // 34.5 — so more than half of all resumes were arithmetically
        // guaranteed to re-starve before the growth actuator (measured
        // 0.68 fr/s) could climb. The two populations in one capture:
        // cap-bound resumes re-starved at a median of **0.33s**, unbound ones
        // at **5.07s**.
        //
        // This is now NetEQ's rule exactly: `kPostponeDecodingLevel = 50`
        // ([decision_logic.cc:29,176-187](decision_logic.cc))
        // holds concealment until the buffer reaches 50% of the target, and
        // that is the whole rule — upstream bounds the *target*
        // (`maximum_delay_ms_`, our `comfort_cap_ms`) and never the resume
        // depth separately. Note the cap only ever bound where
        // `comfort_cap_ms * resume_threshold_pct > 150`: 2.4GHz (800 × 0.5)
        // and Unknown (1000 × 0.25). ADB (100 × 0.2 = 20ms), Ethernet
        // (200 × 0.25 = 50ms) and 5GHz (400 × 0.25 = 100ms) cannot reach
        // 150ms, so this change is algebraically a no-op on all three — see
        // `a_low_latency_link_profile_should_resume_at_the_same_depth_as_before`.
        //
        // `min_depth` remains the outer floor.
        let unpause_threshold =
            ((ctx.target as f32 * self.config.resume_threshold_pct) as u32).max(ctx.min_depth);
        if self.buffer.occupied_count() >= unpause_threshold {
            tracing::info!(
                "[JitterMgr] Prebuffer complete: occupied={}, threshold={}, target={}, max_gap={:.1}",
                self.buffer.occupied_count(),
                unpause_threshold,
                ctx.target,
                self.stats.max_gap_frames(),
            );
            self.flow.is_prebuffering = false;
            // Resume at the top of the controller's own operating band, not
            // at whatever the catch-up burst happened to deliver on the
            // callback that crossed the release threshold.
            //
            // `ingest_packets` drains the whole ring before this test runs,
            // and it runs once per callback — so the occupancy the test sees
            // is post-burst, not the moment the threshold was crossed. The
            // field measured the consequence: **79% of prebuffers
            // overshot, mean +7.6 frames (76ms), max +20 (200ms)**. Some
            // clamp is therefore right: a DTIM burst is not free latency
            // budget.
            //
            // **What it clamps *to* was wrong, and the field measured the
            // cost.** The line was once `unpause_threshold` — one number
            // deciding two unrelated things, *when* to release the playhead
            // and *what depth to keep*. `resume_threshold_pct` is 0.25 (5GHz)
            // / 0.5 (2.4GHz) while the drain floor is
            // `low_limit = 0.75 * target`, so clamping to it landed the buffer
            // **below its own expand trigger by construction, on every
            // resume**:
            //
            // | capture | clamps | discarded | landed below `low_limit` |
            // | --- | --- | --- | --- |
            // | 5GHz uncompressed | 4 | 620ms | 2/4 |
            // | 5GHz 128kbps | 2 | 250ms | 0/2 |
            // | 2.4GHz uncompressed | 28 | **3020ms** | **28/28** |
            // | 2.4GHz 128kbps | 15 | **2490ms** | **15/15** |
            //
            // 6380ms of spliced-away audio, and then the deficit it created:
            // **55.6% / 53.2%** of all below-`low_limit` windows on the two
            // 2.4GHz captures fall within 3s of a resume (73.4% / 77.3% within
            // 6s), and **30.8% / 33.4%** of starvations do. The clamp was
            // manufacturing the underrun the expand actuator then had to fight
            // — which is why growth looked too slow for three rounds running.
            //
            // Clamping to `high_limit` instead — the same `buffer_limits` the
            // drain and the preemptive expand already read, so the resume
            // lands on the one depth where *neither* actuator is armed —
            // recomputes on those same 49 events to **2240ms discarded
            // (-64.9%)** and **11/49 landing below the band** (the residue is
            // resumes that were already below `high_limit` at release, where
            // nothing is discarded and nothing can be). It costs the audio it
            // stops throwing away: 82ms per resume on 2.4GHz uncompressed,
            // 97ms on 2.4GHz 128kbps, 90ms / 10ms on the two 5GHz captures,
            // shed afterwards through the normal drain.
            //
            // `high_limit >= target >= min_depth >= unpause_threshold`
            // whenever `resume_threshold_pct <= 1`, so this can only ever
            // retain *more* than the release threshold; the `.max` states that
            // rather than relying on it.
            //
            // A flush cannot feed `max_gap` → `gap_floor`: `record_gap` is
            // driven purely by `arrival_time` stamped on the receive thread
            // and cannot observe a playback flush. NetEQ has no resume-time
            // flush at all — `kPostponeDecodingLevel = 50`
            // ([decision_logic.cc:29](decision_logic.cc#L29)) waits for the
            // level and then plays, and that is the whole rule.
            //
            // Uses the same `flush_with_crossfade` the startup flush uses, so
            // the discard is spliced, not cut.
            //
            // **Rebuffer exits only, never the initial prebuffer.** Both exits
            // run through this branch, but they are not the same event and the
            // correct depth to resume at differs. A rebuffer resumes into a
            // link whose behaviour the stats have already measured, so the
            // band is an informed depth. The *first* exit resumes on virgin
            // stats — the histogram is still on its geometric seed and the gap
            // window is empty — which is exactly the case the startup flush's
            // 8-frame floor exists for, twenty lines below.
            //
            // Applying the clamp there overrides that floor with a band
            // derived from a target no measurement supports yet: the startup
            // burst went 40 frames -> 2, and
            // `startup_flush_never_leaves_fewer_than_eight_frames` caught it.
            // The startup flush is the authority on the first exit; this is the
            // authority on every later one.
            // **The band is read at `raw_target`, not at `target`.**
            // `target` is the *ramped* value — `advance` walks it toward the
            // measurement at a bounded rate, so on the callback that ends a
            // rebuffer it is still carrying the depth from before the outage.
            // `raw_target` is the controller's own answer to "how deep does
            // this link need to be, right now", already comfort-capped at
            // [`TargetController::target_breakdown`]. Clamping to a band
            // computed from the stale number discards audio the buffer is
            // about to need.
            //
            // Measured on the same 41 resumes the comment above was written
            // from (33 uncompressed + 8 128kbps):
            //
            // | | `high(target)` | `high(max(target, raw))` |
            // | --- | --- | --- |
            // | landed below `max_gap`, unc | 31/33 (93.9%) | 26/33 (78.8%) |
            // | landed below `max_gap`, 128k | **8/8 (100%)** | **4/8 (50%)** |
            // | shortfall vs gap, 128k | 11.75 fr | **1.74 fr** |
            // | discarded, unc / 128k | 660 / 850 ms | **110 / 20 ms** |
            //
            // The margin is causal, not incidental: `(landed - max_gap)`
            // against lines-to-next-starvation is Spearman **r=+0.570,
            // p=0.0005, n=33**, and 73.9% of uncompressed starvations fall
            // within 10 lines of a resume.
            //
            // **The `.max(target)` is load-bearing, not cosmetic.** `raw`
            // falls *below* the ramped target on 11 of 33 uncompressed
            // resumes (the descent phases, where `advance` is walking down
            // behind a gap that has already aged out). `buffer_limits(raw)`
            // alone would clamp lower there. Since `buffer_limits(t).high` is
            // monotone non-decreasing in `t`, taking the max makes this
            // provably one-directional: it can only ever retain more than
            // clamping on `target` alone, never less — verified on all 41
            // events.
            //
            // **What it costs.** Clamping on `target` landed exactly on
            // `high_limit`, the one depth where neither actuator is armed;
            // this can land above it, so the *drain* is armed on resume:
            // 12/33 and 8/8 events, by a mean of 4.58 / 10.38 frames and a
            // worst case of 21. That is shed by the normal drain, whose
            // cooldown here is `timescale_interval` = 2-3 callbacks, so the
            // worst case clears in ~630ms — bought against 118ms of
            // per-resume gap shortfall and the re-starvation it feeds. No
            // event exceeds the comfort cap (max landing 45 against 80);
            // `raw` is capped there already, so that is a property of the
            // formula, not of this sample.
            //
            // This is **not** the `max_gap`-in-the-release-threshold change.
            // `unpause_threshold` above stays a pure function of `target` —
            // `max_gap` is not wired into *when* the playhead is released,
            // only into *how much of an already released burst is kept*. See
            // the warning at the top of this branch.
            let resume_depth = TargetController::buffer_limits(ctx.target.max(ctx.raw_target))
                .high
                .max(unpause_threshold);
            if !self.startup_flush_pending && self.buffer.occupied_count() > resume_depth {
                tracing::info!(
                    "[JitterMgr] Rebuffer clamp: occupied={} -> {} (band_hi, threshold={})",
                    self.buffer.occupied_count(),
                    resume_depth,
                    unpause_threshold,
                );
                self.flush_with_crossfade(resume_depth);
            }
            false
        } else {
            // A total outage entered *during* prebuffering must still be able
            // to reach the stream reset. The normal `missing_count` bookkeeping
            // lives past this early return, so account for it here — otherwise
            // rebuffering after a starvation (see `REBUFFER_AFTER`) would trap
            // the manager in an unbounded silent wait.
            if self.buffer.occupied_count() == 0 {
                self.flow.missing_count += 1;
                if self.check_reset_on_missing() {
                    return true;
                }
            }
            // The playhead is deliberately held, so these callbacks conceal
            // by construction and say nothing about the link. Measuring them
            // would re-trigger the hold the moment it ended.
            self.flow.discard_conceal_window();
            self.generate_plc();
            true
        }
    }

    /// Startup flush: discard burst from OS socket buffer.
    ///
    /// On the very first exit from prebuffering, the ring buffer may contain
    /// a burst of packets that accumulated in the OS socket buffer during
    /// session setup (the streamer starts streaming the moment it receives the
    /// trigger, but the DAC callback hasn’t started consuming yet). Flush
    /// excess down to target depth with a clean crossfade so we start at
    /// optimal latency instead of draining slowly via WSOLA.
    fn apply_startup_flush(&mut self, ctx: &FrameContext) {
        if self.startup_flush_pending {
            self.startup_flush_pending = false;
            // 8 frames (80ms) hard floor. Virgin stats cannot see the link's
            // ordinary 50-90ms delivery gaps yet — the histogram is still on its
            // geometric seed (p95 = 4 frames) and the gap window is empty — so
            // every field log used to starve on the link's *first* ordinary gap
            // and pay a starvation floor plus a 100-200ms plateau to learn what
            // one more startup frame would have covered. 80ms of startup latency
            // probes back down within a minute; the starvation it prevents does
            // not descend, it ratchets.
            let safe_flush_target = ctx.target.max(ctx.min_depth * 2).max(8);
            if self.buffer.occupied_count() > safe_flush_target + 2 {
                tracing::info!(
                    "[JitterMgr] Startup flush: occupied={}, flushing to target={}",
                    self.buffer.occupied_count(),
                    safe_flush_target,
                );
                self.flush_with_crossfade(safe_flush_target);
            }
        }
    }

    /// Static non-zero targets: pin buffer to target depth. Unlike
    /// no-buffer mode (which flushes to target+1 on instantaneous occupancy),
    /// this uses the gentler flush_with_crossfade to keep the decoder warm and
    /// mask the skip. The WSOLA decision band below becomes naturally redundant
    /// (occupied never climbs to high_limit), while expansion still defends
    /// against underrun.
    fn apply_static_target_flush(&mut self, ctx: &FrameContext) {
        let is_static_nonzero = self.config.static_target_ms.is_some_and(|ms| ms > 0);
        if is_static_nonzero && self.buffer.occupied_count() > ctx.target + 1 {
            self.flush_with_crossfade(ctx.target);
        }
    }

    /// Step the playhead past a hole once the reorder tolerance is spent.
    fn try_fast_forward_over_gap(&mut self, ctx: &FrameContext) {
        if self.buffer.occupied_count() > 0 && !self.buffer.has_next() {
            self.flow.gap_hold_count += 1;
            let mut fast_forward_seq = None;

            let tolerance = reorder_tolerance_for(self.network_link, ctx.is_no_buffer);

            if let Some(lo) = self.buffer.lowest_available_seq() {
                let diff = lo.abs_diff(self.buffer.next_play_seq());
                if diff > 20 || self.flow.gap_hold_count >= tolerance {
                    fast_forward_seq = Some(lo);
                }
            } else if self.flow.gap_hold_count >= tolerance {
                self.buffer.advance_one();
                self.log_window.tally.playhead_skips += 1;
                self.log_window.tally.skipped_frames += 1;
                self.flow.gap_hold_count = 0;
                // Skipped a hole with no reordered packet behind it — the next
                // real frame is non-adjacent. Mark it for a fade-in splice.
                self.pending_gap_fadein = true;
            }

            if let Some(lo) = fast_forward_seq {
                let diff = lo.saturating_sub(self.buffer.next_play_seq());
                self.buffer.fast_forward(lo);
                self.log_window.tally.playhead_skips += 1;
                self.log_window.tally.skipped_frames += diff as u32;
                if diff > 20 {
                    self.decoder.resync();
                }
                self.flow.gap_hold_count = 0;
                // Jumped the playhead across a hole; the next frame is
                // discontinuous with what we just played. Fade it in.
                self.pending_gap_fadein = true;
            }
        }
    }

    /// Emit one real decoded frame: verbatim, spliced, or shed.
    fn play_next_frame(&mut self, ctx: &FrameContext) {
        self.flow.gap_hold_count = 0;
        self.flow.missing_count = 0;
        // Every path below this point emits a real decoded frame — verbatim,
        // spliced, or shed — so the concealment run ends here rather than at
        // the `starvation_count` reset further down, which is itself inside a
        // `starvation_count > 0` guard and so cannot end a run that the
        // rebuffer hold produced.
        self.flow.conceal_run = 0;
        self.observe_delivery(false, ctx.now);

        // Apply starvation bump if we just emerged from starvation,
        // but only if the cooldown has expired (prevents ratcheting).
        if self.flow.starvation_count > 0 {
            if !ctx.tcp_mode {
                // Only bump probe_floor on the FIRST starvation event.
                // If starvation_recovery > 0, we're still recovering from a
                // recent starvation — bumping again would cascade the floor
                // upward (observed on 2.4GHz: 155→159→163→...→175 in 300ms).
                if self.flow.starvation_recovery == 0 {
                    self.control.record_starvation(ctx.now);
                    self.control
                        .apply_starvation_floor(&self.config, &self.stats);
                    self.control.jump_to_floor();
                }
                // Refresh the floor-bump cooldown. Scaled to the target: a
                // fixed 500ms is shorter than a 2.4GHz DTIM cycle, so the
                // guard expired mid-cascade and the floor got re-bumped —
                // three "Starvation floor set" logs inside 3s in the field log.
                // This countdown is re-armable *because* its job is to span a
                // whole starvation cluster; the time-stretch gate below is a
                // separate, wall-clock-bounded window for exactly that reason.
                self.flow.starvation_recovery = self
                    .control
                    .effective_target
                    .saturating_mul(2)
                    .clamp(50, 200);
                // Suppress WSOLA for 500ms from the *first* starvation of this
                // episode only — do not extend it on every recovery.
                self.flow.arm_recovery_window(ctx.now);
            }
            // Always reset — prevents permanent fade-in loop in TCP/ADB mode.
            self.flow.starvation_count = 0;
        }

        let pkt = self.buffer.pop_next().expect("has_next was true");
        self.log_window.frames_played += 1;
        let delay_ms = ctx.now.duration_since(pkt.arrival_time).as_millis() as u32;
        self.latency_metric.store(delay_ms, Ordering::Relaxed);
        // Mirror the network jitter estimate out on the same cadence. `ema_jitter`
        // is in frames; scale to whole ms. Instrumentation only — no alloc/block/log.
        let jitter_ms = (self.stats.ema_jitter * MILLIS_PER_FRAME as f32).round() as u32;
        self.jitter_metric.store(jitter_ms, Ordering::Relaxed);
        // Keep the frame that is about to be overwritten as `expand`'s
        // "already played" half (NetEQ's `old_data`). It has to be captured
        // here, before `capture` clobbers `decode_buf`, and it has to happen on
        // every callback rather than only when a splice fires: the expand rate
        // limiter is 20 callbacks, so a lazier update would hand the search a
        // 200ms-stale neighbour and destroy the correlation it measures.
        self.timescale.remember(self.decoder.decoded());
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
            // Same discontinuity, stated to the time-scaler: the remembered
            // frame is no longer adjacent to this one, and `expand` replays
            // its tail out of that history. Splicing across the seam would
            // repeat stale audio instead of one pitch period. Dropping it
            // costs only the width of the search — one frame still reaches
            // [OLA_LEN, 352], which is non-degenerate by construction.
            self.timescale.forget();
        }
        self.pending_gap_fadein = false;

        if self.run_timescale(ctx) {
            return;
        }

        self.playback_buf
            .extend(&self.decoder.decode_buf[..self.decoder.decode_len]);
    }

    /// Drive the filtered buffer level toward the target with WSOLA.
    ///
    /// Returns `true` when a branch emitted this callback's audio itself, so
    /// the caller must not also emit the frame verbatim.
    ///
    /// NetEQ decision band (`DecisionLogic::ExpectedPacketAvailable`).
    /// The operating point IS the target. We compute a decision band around
    /// it and drive the filtered buffer level toward `target`:
    ///   filtered >= emergency → fast accelerate  (emergency drain, no cooldown)
    ///   filtered >= high      → normal accelerate (gentle drain, cooldown-gated)
    ///   filtered <  low       → preemptive expand (slow down)
    /// Unlike the old design there is no `target+2` floor, no 3×/5× flush
    /// ceiling, and no RMS gate on accelerate — transparency is guaranteed by
    /// the WSOLA correlation gate (0.9 normal / 0.5 fast), exactly as NetEQ.
    ///
    /// After any stretch we immediately correct the filtered level by the
    /// number of frames added/removed (NetEQ's BufferLevelFilter time-stretch
    /// compensation). Without this the α≈0.99 filter lags ~1.3s and the drain
    /// decision oscillates or stalls — the root cause of the 2.4GHz plateau.
    fn run_timescale(&mut self, ctx: &FrameContext) -> bool {
        let Band {
            low: low_limit,
            high: high_limit,
        } = TargetController::buffer_limits(ctx.target);
        let filtered = self.flow.filtered_buffer_level;

        // NetEQ suppresses time-stretching for one frame right after an expand
        // (prev_mode == kModeExpand); our analogue is a 500ms wall-clock window
        // from the first starvation of an episode. Both prevent the
        // drain→starve→refill saw-tooth, and the wall clock is what stops a
        // starvation cluster from holding the actuators off indefinitely.
        let stretch_allowed = self.flow.stretch_allowed(ctx.now);

        // Signal energy of the frame we're about to play. Still computed, still
        // logged, and still the gate on the two paths that have no correlation
        // check of their own (the silence fast-forward below, and free growth
        // on silence) — but it is no longer a *precondition* for attempting a
        // WSOLA accelerate.
        //
        // It was, for four rounds, and a field census is what retired it:
        // `declined_rms_mask` took 93-96% of every drain attempt on all three
        // links — 36 455 attempts, 243 splices, over 881 seconds — while
        // `declined_recovery` was zero everywhere and `declined_cooldown` under
        // 4%. `ARTIFACT_MASK_RMS` is -22dBFS, so on program material the gate
        // was not filtering splices, it was a closed valve. The buffer parked
        // at 99ms against a 76ms target on ADB with a 310ms peak, and starvation
        // went 0 → 14 episodes on a *cable*, because the drain could not act
        // and the growth path could not either.
        //
        // Two further reasons it had to go rather than be retuned:
        //
        //  * Its justification expired. The comment it carried argued the OLA is
        //    audible "even at NCC ≥ 0.9" — written when `ACCEL_WINDOW_FRAMES`
        //    was 1, where the pitch search reached only 136-375Hz and so could
        //    not find a true period in bass or dense mixes at *any* testable
        //    lag. The gate was the workaround for that geometry. Two frames
        //    reach 67-375Hz (upstream's 66-400Hz) and the workaround was never
        //    revisited.
        //  * It inverts masking. Admitting splices only on isolated quiet frames
        //    concentrates every seam in exactly the content least able to hide
        //    one. A splice under a loud passage is buried; the same splice in a
        //    lull is exposed.
        //
        // What remains is upstream's own structure (`accelerate.cc:58`):
        // `(best_correlation > threshold) || !active_speech` — an OR, where the
        // correlation check is the quality gate and the VAD is an *escape*, not
        // a precondition. Our correlation gate is already NetEQ-exact
        // (`fast_mode ? 0.5 : 0.9`, `timescale.rs`) and `declined_ncc` measured
        // 0.8-2.6% of attempts, so it is a live veto and not a rubber stamp.
        //
        // This is not the removal that once regressed this module. That was
        // deleting the masking gate and leaving *nothing* in its place; the
        // splice remains gated on correlation, and the RMS is still measured and
        // now reported per window (`avg_rms`/`max_rms`) so it can come back as a
        // relative VAD if the field says correlation alone is not enough.
        //
        // The standing risk is splice *geometry*, not the gate: `OLA_LEN` is 128
        // samples (2.67ms) against upstream's `fs_mult_ * 120` (15ms), 5.6x
        // shorter and so more exposed at the same correlation. If the field
        // reports a warble or flutter on sustained tones — as distinct from the
        // PLC buzz this is meant to remove — the answer is a longer crossfade,
        // not this gate again.
        let rms = Self::get_rms(&self.decoder.decode_buf[..self.decoder.decode_len]);
        self.log_window.tally.rms_sum += rms;
        self.log_window.tally.rms_count += 1;
        self.log_window.tally.rms_max = self.log_window.tally.rms_max.max(rms);

        // Census of *why* a drain did not happen, not merely that it did
        // not. Every branch below is unchanged; these are increments only.
        let over_high = filtered >= high_limit as f32;
        if over_high && !stretch_allowed {
            self.log_window.tally.declined_recovery += 1;
        }

        if stretch_allowed && over_high {
            let is_fast = filtered >= emergency_threshold(high_limit) as f32;
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
                        self.log_window.tally.shed += 1;
                    }
                }
                // The shed deleted frames without emitting them, so history is
                // no longer adjacent to what will be played next.
                self.timescale.forget();
                self.timescale_cooldown = timescale_interval(ctx.target);
                return true;
            }

            // Fast accelerate bypasses the cooldown as well (NetEQ
            // kFastAccelerate) and lowers the correlation threshold from 0.9
            // to 0.5 inside `accelerate`; the normal tier still waits out the
            // rate limiter. The masking gate is no longer a precondition on
            // either — see the census above — so the correlation check inside
            // `accelerate` is what decides whether this splice happens.
            let masked = rms < ARTIFACT_MASK_RMS;
            if is_fast || self.timescale_cooldown == 0 {
                // Stage the frame about to be played, then extend the window
                // with the next contiguous frame (up to 20ms) so the pitch
                // search can reach periods longer than one frame. Every staged
                // frame must be emitted exactly once — via the returned splice
                // on success, or verbatim when the splice declines.
                //
                // No filtered-level credit for the extra pop: `filter_buffer_level`
                // ticks at the top of `process_next_frame`, which only runs once
                // `playback_buf` has drained empty, so every tick already sees an
                // occupancy with no staged audio in flight behind it.
                let mut window = self.timescale.window_begin(self.decoder.decoded());
                if self.buffer.has_next() && window.headroom() >= self.decoder.decode_len {
                    let next_pkt = self.buffer.pop_next().expect("has_next was true");
                    self.decoder.capture(&next_pkt);
                    window.extend(self.decoder.decoded());
                    // Emitted below on both outcomes, but not through
                    // `frames_played` — which counts callbacks, not packets.
                    // Counted here so the `unplayed` ledger can subtract it
                    // instead of reporting it as loss.
                    self.log_window.tally.staged_pops += 1;
                }
                let window_frames = window.staged().len() / OPUS_FRAME_SAMPLES;
                let spliced = window.accelerate(is_fast, rms, &mut self.playback_buf);
                if let Some(removed_samples) = spliced {
                    // Immediately debit the removed audio from the filtered level.
                    let removed_frames = removed_samples as f32 / OPUS_FRAME_SAMPLES as f32;
                    self.flow.adjust_filtered_level(-removed_frames);
                    self.log_window.tally.accelerated += 1;
                    if is_fast {
                        self.log_window.tally.fast_accelerated += 1;
                    }
                    self.log_window.tally.removed_frames += removed_frames;
                    tracing::trace!(
                        "[JitterMgr] Accelerate: filtered={:.1}, target={}, high={}, fast={}, removed_frames={:.2}, window_frames={}",
                        filtered,
                        ctx.target,
                        high_limit,
                        is_fast,
                        removed_frames,
                        window_frames,
                    );
                    if !is_fast {
                        self.timescale_cooldown = timescale_interval(ctx.target);
                    }
                    // The splice the old masking gate would have refused. This
                    // is the count that makes the demotion's risk falsifiable: if
                    // the field reports a warble on sustained tones, this says
                    // how many loud splices produced it, and if it reports
                    // nothing, this says how many were transparent. Reported
                    // per window beside `avg_rms`.
                    if !masked {
                        self.log_window.tally.loud_splices += 1;
                    }
                    self.note_splice_quality(rms);
                } else {
                    self.log_window.tally.declined_ncc += 1;
                    window.emit(&mut self.playback_buf);
                }
                return true;
            }
            // Over `high_limit`, off the recovery guard, and still no drain.
            // Since the masking gate stopped being a precondition, the cooldown
            // is the only remaining way to land here — `declined_rms_mask` is
            // kept, and kept printing, precisely so that a non-zero reading
            // would say the gate came back by some path nobody intended. It
            // must read 0 on every link.
            if self.timescale_cooldown > 0 {
                self.log_window.tally.declined_cooldown += 1;
            }
        } else if filtered < low_limit as f32 {
            // --- Below target: grow the buffer ---
            // Growth is where the click train lived. The old code sent every
            // below-target callback straight to WSOLA `expand`, which inserts
            // exactly ONE pitch period per call — so a target that jumped 15
            // frames produced a splice every 60ms for several seconds. Growth
            // now has two paths, and neither of them is a splice train:
            // (a) Free growth on silence. The frame is already below
            // -46dBFS, so appending a frame of true silence after it is
            // inaudible — no correlation search, no seam, no artifact.
            // This is the mirror of the silence fast-forward *drain*
            // shortcut above, and it is where most growth should happen.
            //
            // Two guards, both load-bearing:
            //  * REAL occupancy below target, not just the filtered level.
            //    The IIR takes ~1.3s to converge, so right after
            //    prebuffering `filtered` reads far below `low_limit` while
            //    the buffer is in fact full. Growing on that would buy
            //    latency to fix a filter lag rather than a real shortfall.
            //  * Adaptive mode only. A static / no-buffer target is the
            //    user pinning the depth by hand; padding silence there
            //    would fight the static flush a few lines above, which
            //    exists precisely to hold the buffer AT that depth.
            if rms < SILENCE_RMS
                && self.timescale_cooldown == 0
                && self.config.static_target_ms.is_none()
                && self.buffer.occupied_count() < ctx.target
            {
                let silence_len = self.decoder.decode_len;
                self.playback_buf
                    .extend(&self.decoder.decode_buf[..silence_len]);
                self.playback_buf
                    .extend(std::iter::repeat_n(0.0, silence_len));
                self.flow.adjust_filtered_level(1.0);
                self.log_window.tally.grown += 1;
                self.timescale_cooldown = MIN_SILENCE_GROW_INTERVAL;
                return true;
            }

            // (b) WSOLA expand, on two triggers with different urgency.
            //
            // It is no longer gated on content energy either, and the reason
            // is that the alternative here is not silence, it is PLC.
            //
            // The field census measured what the energy gate cost: `expand`
            // fired **7 times in 881 seconds** across all three links (5GHz 2,
            // ADB 1, 2.4GHz 4), because it needed `occupied <= 1` *and* an RMS
            // inside `[SILENCE_RMS, ARTIFACT_MASK_RMS)` — a window that
            // program material passes through on its way between passages, not
            // one it sits in. So the underrun defence was unavailable at
            // exactly the moments it was built for, and the buffer starved
            // instead: ADB went 0 → 14 starvation episodes on a *cable*, 5GHz
            // 9 → 29.
            //
            // What starvation runs instead is `generate_plc`, which does not
            // begin fading until `conceal_run > 3` — so the first three
            // frames of every episode are raw Opus PLC extrapolation. 50% of
            // ADB's episodes and 34% of 5GHz's were ≤30ms, i.e. entirely
            // inside that unfaded band. That is the "electric-buzz like noise"
            // the field reported on all three links including the cable.
            //
            // Weighed correctly, the trade is not "one WSOLA splice vs.
            // nothing": it is one pitch-period insert, gated at NCC 0.9 inside
            // `expand` and rate-limited to one per 200ms, against a whole frame
            // of unfaded extrapolation with no correlation check at all. The
            // splice is the quieter of the two on any content, and the louder
            // the content the more true that is — loud material masks a seam
            // and does nothing to mask PLC's spectral drift.
            //
            // --- the buffer had no way to reach its own target ---
            //
            // `occupied <= 1` was once the *only* trigger, which made expand
            // a last-ditch underrun defence and nothing else. Field captures
            // say that is not enough: on 2.4GHz the occupancy sat **15.7 frames
            // below target in 85% of screen-off windows** while arrivals matched
            // playback exactly (99.6/s both ways), so nothing accumulated and
            // nothing grew the buffer. The only mechanism that could raise the
            // depth was starving — the stutter itself — and starvation went
            // 16 → 57 on that link.
            //
            // The missing branch is upstream's growth half of the decision band
            // (`decision_logic.cc:294-295`):
            //
            //     if (buffer_level_filter_->filtered_current_level() < low_limit)
            //       return kPreemptiveExpand;
            //
            // We ported the drain half and the band that frames it, but never
            // this. Measured against those logs it would arm on **50.3%**
            // of 2.4GHz windows against our 3.4%, and on 8-9% of ADB and 5GHz
            // windows — small where the buffer already tracks its target, large
            // exactly where it does not.
            //
            // Two triggers, both rate-limited by `MIN_EXPAND_INTERVAL`:
            //
            //  * `occupied <= 1` — imminent underrun, the last-ditch defence.
            //  * `filtered < low_limit` — preemptive growth, upstream's band
            //    condition.
            //
            // **Neither is cooldown-exempt, and the obvious reading gets that
            // wrong.** Exempting the imminent-underrun tier looks
            // right — "a rate limiter that suppresses the defence at the moment
            // it is needed is worse than no limiter" — but a buffer held at one
            // frame by a rate-matched trickle satisfies `occupied <= 1` on
            // *every* callback, so the exemption is not a last-ditch escape, it
            // is an unbounded splice train. Measured by
            // `expand_should_stay_rate_limited_when_the_buffer_hovers_at_one_frame`:
            // **71 inserts in 400 callbacks, ~17.8Hz** — the same "fast clicking
            // on every buffer increase" that `MIN_EXPAND_INTERVAL` was
            // introduced to stop, arriving by a new route.
            //
            // The exemption was also never a port: upstream rate-limits
            // `kPreemptiveExpand` through `kMinTimescaleInterval` with no
            // equivalent bypass. NetEQ's *unlimited* concealment path is
            // `kExpand`, which is our `generate_plc` on genuine starvation
            // (`occupied == 0`), not this.
            //
            // What actually fixes the measured starvation is the preemptive
            // trigger, which grows the buffer while it is still in the band and
            // therefore keeps it from reaching one frame at all. The
            // imminent-underrun tier stays as a floor under that, at one splice
            // per 200ms.
            //
            // This is the mechanism `MIN_EXPAND_INTERVAL` was introduced to stop
            // becoming a growth path, and the comment above says so. The
            // difference is the gate: `filtered < low_limit` is a *band*
            // condition that closes the moment the buffer reaches the band,
            // whereas the earlier version had no band and grew unconditionally
            // on every below-target callback. Bounded at one pitch period per
            // 200ms and NCC-gated, growth is now slower than the drain it is
            // balancing against.
            let imminent_underrun = self.buffer.occupied_count() <= 1;
            let preemptive_trigger = filtered < low_limit as f32;
            // A static target is the user pinning the depth by hand, and the
            // pin is enforced by the `flush_with_crossfade` above, which acts
            // on *instantaneous* occupancy. `filtered` is an IIR with a ~1.3s
            // time constant, so for several callbacks after each flush it still
            // reads the pre-flush level and `preemptive_trigger` stands while
            // the buffer is in fact exactly at target. Growth on that reading
            // fights the pin: measured as a static 60ms target parking at 25
            // frames, because each expand parks surplus in `playback_buf` and
            // the next callback then skips `process_next_frame` — where the
            // static flush lives. Free silence growth already carries the same
            // guard, for the same reason (see `static_target_ms.is_none()`
            // above); the drain does not need one because the flush reaches
            // target before the drain's `filtered >= high_limit` can arm.
            if stretch_allowed
                && self.timescale_cooldown == 0
                && (imminent_underrun || preemptive_trigger)
                && self.config.static_target_ms.is_none()
            {
                // `expand` searches `[remembered previous frame | this frame]`
                // and emits only this frame plus the inserted period. Upstream's
                // input contract is the same shape — `PreemptiveExpand::Process`
                // takes `[old_data | new_data]` totalling ~30ms, where
                // `old_data` is already-played audio borrowed back from the sync
                // buffer (`preemptive_expand.cc:28-33`).
                //
                // Staging the *next packet* instead was the first draft and it
                // is wrong twice over: it disables the actuator exactly at
                // `occupied <= 1`, the imminent-underrun case the tier exists
                // for, because there is no next packet to stage; and it emits
                // two frames per call, parking enough surplus in `playback_buf`
                // that the following callback skips `process_next_frame`
                // entirely — ingest, depth control, the drain and the static
                // flush all live there. History costs neither: it is always
                // available, and emission stays one frame plus one period.
                // **Two upstream operations, selected by tier.** At
                // `imminent_underrun` this is NetEQ's `Expand` (`expand.cc`),
                // which carries *no* correlation gate — it always conceals,
                // because the alternative at `occupied <= 1` is a hole, not a
                // worse seam. Below that it is `PreemptiveExpand`, which keeps
                // its 0.9 NCC gate because a packet is still in hand and the
                // splice can afford to wait for a good seam. Running both
                // through the gated path made the underrun tier refuse 83 of
                // 90 attempts on 2.4GHz uncompressed. See `expand_conceal`.
                let spliced = if imminent_underrun {
                    self.timescale.expand_conceal(
                        self.decoder.decoded(),
                        rms,
                        &mut self.playback_buf,
                    )
                } else {
                    self.timescale
                        .expand(self.decoder.decoded(), rms, &mut self.playback_buf)
                };
                if let Some(inserted_samples) = spliced {
                    let inserted_frames = inserted_samples as f32 / OPUS_FRAME_SAMPLES as f32;
                    self.flow.adjust_filtered_level(inserted_frames);
                    self.log_window.tally.expanded += 1;
                    if !imminent_underrun {
                        self.log_window.tally.preemptive += 1;
                    }
                    self.log_window.tally.inserted_frames += inserted_frames;
                    self.timescale_cooldown = MIN_EXPAND_INTERVAL;
                    self.note_splice_quality(rms);
                    tracing::trace!(
                        "[JitterMgr] Expand: filtered={:.1}, low={}, occupied={}, imminent={}, inserted_frames={:.2}",
                        filtered,
                        low_limit,
                        self.buffer.occupied_count(),
                        imminent_underrun,
                        inserted_frames,
                    );
                    return true;
                }
                if !imminent_underrun {
                    // The preemptive attempt reached `expand` and its own NCC
                    // gate refused. Counted separately from `declined_ncc` (the
                    // drain side) so the two actuators stay attributable — a
                    // preemptive path that arms constantly and never splices
                    // looks identical to one that never arms unless this is
                    // counted.
                    self.log_window.tally.declined_preemptive_ncc += 1;
                } else {
                    // Same refusal, one frame from empty. Long uncounted, which
                    // made the tier that matters most the only one the census
                    // could not see.
                    //
                    // **This is now unreachable on quality grounds.** The
                    // concealment tier no longer applies an NCC gate at all
                    // (`expand_conceal`), so the only remaining `None` returns
                    // are geometric — a window too short to hold a reference
                    // plus a crossfade, or a search range that collapsed — and
                    // none of those are reachable at a 480-frame packet. A
                    // non-zero reading in the field therefore says the gate came
                    // back by a path nobody intended, in exactly the way
                    // `declined_rms_mask` does.
                    self.log_window.tally.declined_underrun_ncc += 1;
                }
                // A declined splice wrote nothing, so the frame falls through
                // to the verbatim emit below. `expand` popped no packet, so
                // there is no staged audio to account for either.
            }
        }

        false
    }

    /// No frame is available: count the miss and conceal.
    fn handle_missing_frame(&mut self, ctx: &FrameContext) {
        self.flow.missing_count += 1;
        self.observe_delivery(true, ctx.now);

        if self.buffer.occupied_count() == 0 {
            self.flow.gap_hold_count = 0;
            self.flow.starvation_count += 1;
            if self.flow.starvation_count == 1 {
                // Snap the filtered level to the truth. The IIR time constant is
                // ~1.3s at a large target, so without this the drain/expand
                // decision spends the whole recovery acting on a pre-starvation
                // reading of a buffer that is, right now, empty.
                self.flow.filtered_buffer_level = 0.0;
                // One line per *episode*, not per onset: under a trickle the
                // onsets recur every ~3 callbacks, which once printed 299 identical
                // warnings in 9.4s. The closing census (see `observe_delivery`)
                // carries the count and duration those repeats stood for.
                if self.flow.note_starvation_onset(ctx.now) {
                    tracing::warn!(
                        "[JitterMgr] Starvation started: effective_target={}, probe_floor={}, max_gap={:.1}, ema_jitter={:.2}, burst={}",
                        self.control.effective_target,
                        self.control.probe_floor(),
                        self.stats.max_gap_frames(),
                        self.stats.ema_jitter,
                        self.stats.burst_detected(),
                    );
                }
            }
            if self.flow.starvation_count == REBUFFER_AFTER {
                tracing::info!(
                    "[JitterMgr] Rebuffering after starvation: effective_target={}, max_gap={:.1}",
                    self.control.effective_target,
                    self.stats.max_gap_frames(),
                );
                self.flow.is_prebuffering = true;
            }
        }

        if self.check_reset_on_missing() {
            return;
        }

        self.generate_plc();
    }

    /// Record this callback's delivery outcome, close a starvation episode when
    /// delivery has genuinely recovered, and hold the playhead when a whole
    /// window was mostly concealed.
    ///
    /// The failure this exists for: `REBUFFER_AFTER` counts *consecutive* empty
    /// callbacks, and `starvation_count` is zeroed by every pop. Under sustained
    /// under-delivery — a packet every ~3rd callback — it therefore never fires.
    /// A measured 5GHz storm ran 9.4s that way: 299 starvation onsets, **one**
    /// rebuffer, zero resets, and one real frame between every 2-3 PLC frames for
    /// the rest of the stream. The rebuffer is the right actuator (its own doc
    /// comment argues the case: one clean gap beats six audible ones); it was
    /// simply unreachable. A ratio over a tumbling window can see a deficit that
    /// is punctuated by the very arrivals that constitute it.
    ///
    /// Note the honest limit: on a link delivering below real time, no buffer
    /// depth produces continuous audio. This trades many short stutters for
    /// fewer, longer holds. The field call is whether that is the better artifact.
    fn observe_delivery(&mut self, concealed: bool, now: Instant) {
        // An episode ends only when a pop leaves the buffer genuinely healthy.
        // Ending it on any pop would re-open it on the next callback and restore
        // the per-onset log storm this replaces.
        if !concealed
            && self.buffer.occupied_count() > self.min_depth_frames()
            && let Some((events, elapsed)) = self.flow.close_starvation_episode(now)
        {
            tracing::warn!(
                "[JitterMgr] Starvation episode ended: events={}, duration={}ms, effective_target={}, occupied={}",
                events,
                elapsed.as_millis(),
                self.control.effective_target,
                self.buffer.occupied_count(),
            );
        }

        // While the playhead is already held, every callback conceals by
        // construction. Counting those would re-trigger the hold the instant it
        // ended, which is a latch.
        if self.flow.is_prebuffering {
            self.flow.discard_conceal_window();
            return;
        }

        let Some(verdict) = self.flow.observe_delivery(concealed) else {
            return;
        };
        if verdict.should_rebuffer {
            tracing::warn!(
                "[JitterMgr] Sustained under-delivery: conceal={}% of {} callbacks, effective_target={}, occupied={}, starvations={} — holding playhead",
                verdict.conceal_pct,
                verdict.callbacks,
                self.control.effective_target,
                self.buffer.occupied_count(),
                self.flow.starvation_events,
            );
            self.flow.is_prebuffering = true;
        } else if verdict.conceal_pct > 0 {
            tracing::info!(
                "[JitterMgr] Delivery window: conceal={}% of {} callbacks, effective_target={}, occupied={}",
                verdict.conceal_pct,
                verdict.callbacks,
                self.control.effective_target,
                self.buffer.occupied_count(),
            );
        }
    }

    /// Shared `missing_count` → stream-reset check. Returns `true` when a reset
    /// was triggered and the caller must return immediately (a frame of silence
    /// has already been queued).
    ///
    /// Extracted so the prebuffer early-return path can run it too: without that,
    /// a total outage entered during rebuffering would never reach the reset.
    fn check_reset_on_missing(&mut self) -> bool {
        if self.flow.missing_count > Self::max_missing_for(self.network_link) {
            self.trigger_reset();
            self.playback_buf
                .extend(std::iter::repeat_n(0.0, OPUS_FRAME_SAMPLES));
            return true;
        }
        false
    }

    /// Once-per-second `info!` naming every input to the depth decision and which
    /// term won it, plus the delivery deficit over the same second.
    ///
    /// At `info!` rather than `debug!` on purpose: the mobile crate installs
    /// `LevelFilter::Info` (`gemacast-mobile/src-tauri/src/lib.rs`), so every
    /// `debug!` in this module is invisible in `adb logcat`. Three rounds of field
    /// diagnosis had to reconstruct `raw_target` by hand from `max_gap` and the link
    /// profile, and misattributed the 5GHz storm twice before the arithmetic ruled
    /// the histogram in. One line per second is ~0.1% of the callback rate.
    ///
    /// `frames_played` counts *callbacks that consumed a frame*, not packets, so
    /// it cannot stand in for the packet rate: during a measured 2.4GHz collapse
    /// both fell together (arrivals 48, played 50) and `arrivals < played` never
    /// tripped in two consecutive field rounds. Under-delivery is therefore
    /// measured against **wall clock** — the nominal packet rate over the window
    /// the log line actually spanned — which fires on 24 of 275 2.4GHz windows
    /// and on none of the 205 ADB / 257 5GHz ones. `played` stays in the line as
    /// the playback-side half of the same picture.
    ///
    /// `window_ms` is printed because it is not a constant. It held 1000±20ms on
    /// ADB and 5GHz but stretched to 1120-1250ms throughout the 2.4GHz collapse
    /// and to 3161ms at its worst, so every per-window count in this line is
    /// only interpretable next to the span it was accumulated over.
    ///
    /// `accel`/`expand`/`shed`/`grown` and the `declined_*` census exist because
    /// the timescale layer logs its splices at `trace!` and the mobile crate
    /// installs `LevelFilter::Info` — across three field captures the layer that
    /// performs every edit to the audio emitted **not one line**. See
    /// [`TimescaleTally`].
    ///
    /// `burst_floor` and `inter_burst_gap` are still printed but are **no longer a
    /// depth authority** — `winning_term` can never name them. They stay
    /// in the line because the divergence between them and `max_gap` is what
    /// diagnosed the cluster-anchor defect (ADB: 5783.5 vs 24), and a term removed
    /// from the arithmetic but dropped from the log is a term that can quietly
    /// return.
    ///
    /// `max_gap_age` is printed next to `max_gap` because the level alone is
    /// ambiguous: a flat `max_gap` means either one gap riding its flat-top or a
    /// gap recurring inside it, and those call for opposite responses. A 5GHz
    /// capture held `max_gap` at ~21 frames for 73s with
    /// `arrivals` at 99-104/s, and nothing in the log could say which it was. See
    /// [`super::stats::JitterStats::max_gap_age_secs`].
    fn log_depth_authority(&mut self, breakdown: &TargetBreakdown, target: u32, now: Instant) {
        self.log_window.frame_count += 1;
        if self.log_window.frame_count < LOG_INTERVAL_CALLBACKS {
            return;
        }
        let arrivals = self.stats.take_arrival_count();
        let (played, tally, window) = self.log_window.rotate(now);
        // Nominal frame count for the elapsed wall clock. Falls back to the
        // callback count for the very first window, which has no prior anchor.
        let window_ms = window.map_or(LOG_INTERVAL_CALLBACKS * MILLIS_PER_FRAME, |d| {
            d.as_millis() as u32
        });
        let expected = Self::expected_packets(window_ms);

        let Band {
            low: low_limit,
            high: high_limit,
        } = TargetController::buffer_limits(target);
        tracing::info!(
            "[JitterMgr] Depth: effective_target={}, ramp_goal={}, raw_target={}, pre_clamp={:.1}, winning_term={}, probe_floor={}, filtered={:.1}, occupied={}, low_limit={}, band={}..{}, min_depth={:.0}, histogram={:.1}, burst_floor={:.1}, gap_floor={:.1}, max_gap={:.1}, max_gap_age={}s, inter_burst_gap={:.1}, burst={}, arrivals={}, played={}, staged={}, unplayed={}, stale={}, stale_lag_avg={:.1}, stale_lag_max={}, skips={}, skipped_frames={}, flush_discards={}, window_ms={}, accel={}, fast_accel={}, expand={}, preempt={}, shed={}, grown={}, removed_frames={:.2}, inserted_frames={:.2}, declined_cooldown={}, declined_rms_mask={}, declined_recovery={}, declined_ncc={}, declined_preempt_ncc={}, declined_underrun_ncc={}, loud_splices={}, avg_rms={:.4}, max_rms={:.4}, splice_rms={:.4}, splice_step={:.2}, conceal_run={}, pitch_conceals={}, conceal_step={:.2}, floor_frames={}, fmt={}",
            self.control.effective_target,
            self.control.ramp_goal,
            breakdown.raw,
            breakdown.pre_clamp,
            breakdown.winning.as_str(),
            self.control.probe_floor(),
            self.flow.filtered_buffer_level,
            self.buffer.occupied_count(),
            low_limit,
            low_limit,
            high_limit,
            breakdown.min_depth,
            breakdown.histogram_base,
            breakdown.burst_floor,
            breakdown.gap_floor,
            self.stats.max_gap_frames(),
            self.stats.max_gap_age_secs(),
            self.stats.inter_burst_gap_frames(),
            self.stats.burst_detected(),
            arrivals,
            played,
            tally.staged_pops,
            // The ledger, printed alongside its components so a residual is
            // visible in one line instead of being reconstructed across three.
            // `staged` is subtracted rather than merely printed: those packets
            // were emitted, so counting them as unplayed reported 6.8% packet
            // loss on a capture whose true loss was zero.
            arrivals.saturating_sub(played + tally.staged_pops),
            tally.stale_rejects,
            if tally.stale_rejects > 0 {
                tally.stale_lag_sum as f32 / tally.stale_rejects as f32
            } else {
                0.0
            },
            tally.stale_lag_max,
            tally.playhead_skips,
            tally.skipped_frames,
            tally.flush_discards,
            window_ms,
            tally.accelerated,
            tally.fast_accelerated,
            tally.expanded,
            tally.preemptive,
            tally.shed,
            tally.grown,
            tally.removed_frames,
            tally.inserted_frames,
            tally.declined_cooldown,
            tally.declined_rms_mask,
            tally.declined_recovery,
            tally.declined_ncc,
            tally.declined_preemptive_ncc,
            tally.declined_underrun_ncc,
            tally.loud_splices,
            if tally.rms_count > 0 {
                tally.rms_sum / tally.rms_count as f32
            } else {
                0.0
            },
            tally.rms_max,
            if tally.splice_rms_count > 0 {
                tally.splice_rms_sum / tally.splice_rms_count as f32
            } else {
                0.0
            },
            tally.splice_step_max,
            tally.conceal_run_max,
            tally.pitch_conceals,
            tally.conceal_step_max,
            tally.floor_frames,
            self.decoder.last_format.as_str(),
        );

        // Sustained under-delivery: packets are arriving, so no reset detector or
        // starvation counter reads it as an outage, but they arrive slower than the
        // DAC drains them. No target depth fixes this — it is the one condition
        // where the honest answer is "the link cannot carry the stream".
        if expected > 0 && Self::is_under_delivering(arrivals, expected) {
            tracing::warn!(
                "[JitterMgr] Under-delivery: {} arrivals against {} expected over {}ms ({:.0}% of nominal, {} frames played) — the link is not carrying the stream",
                arrivals,
                expected,
                window_ms,
                arrivals as f32 / expected as f32 * 100.0,
                played,
            );
        }
    }

    /// Flush buffer down to `flush_to` frames with a WSOLA crossfade across
    /// the skip boundary. Keeps the decoder state warm by decoding every
    /// skipped packet (output is discarded), then splices the pre-flush and
    /// post-flush audio with the existing OLA crossfade ramp.
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
        // Every frame between here and `flush_to` leaves the pipeline without
        // reaching the DAC. All three callers log their own line, but only this
        // counter puts them in the same ledger as the stale and skip sinks.
        self.log_window.tally.flush_discards += self.buffer.occupied_count() - flush_to;
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
        // Flush the episode census before the counters go, so a starvation
        // cluster that ends in a reset is still reported rather than silently
        // reattributed to the next stream.
        if let Some((events, elapsed)) = self.flow.close_starvation_episode(Instant::now()) {
            tracing::warn!(
                "[JitterMgr] Starvation episode ended at reset: events={}, duration={}ms",
                events,
                elapsed.as_millis(),
            );
        }
        tracing::warn!(
            "[JitterMgr] Stream reset: missing_count exceeded {}ms silence threshold",
            Self::max_missing_for(self.network_link) * MILLIS_PER_FRAME,
        );
        self.buffer.reset();
        self.flow.reset_on_stream_restart();
        self.playback_buf.clear();
        self.decoder.reset();
        self.stats.reset_on_stream_restart();
        self.control.reset();
        self.pending_gap_fadein = false;
        // The remembered frame belongs to the stream that just ended.
        self.timescale.forget();
    }

    fn get_rms(samples: &[f32]) -> f32 {
        let mut sum_sq = 0.0;
        for &s in samples {
            sum_sq += s * s;
        }
        (sum_sq / samples.len() as f32).sqrt()
    }

    fn generate_plc(&mut self) {
        // Count the run here rather than at the two call sites, because the two
        // sites are exactly what `starvation_count` cannot span: the starvation
        // path increments it, the rebuffer hold's early return does not, and the
        // hold is where the longest runs live.
        self.flow.conceal_run = self.flow.conceal_run.saturating_add(1);
        self.log_window.tally.conceal_run_max = self
            .log_window
            .tally
            .conceal_run_max
            .max(self.flow.conceal_run);

        // Conceal from what actually played when the codec has no state to
        // extrapolate from. On the uncompressed and silence paths `capture` never
        // feeds the decoder, so `decode_plc` there runs on a decoder this stream
        // has never advanced and returns **exact zeros** — every one of the 267
        // concealed frames (2674ms) of an uncompressed field capture was digital
        // silence, which is a hole with a fade on it, not concealment.
        //
        // `conceal_frame` writes a full frame and returns `false` having written
        // nothing when it cannot, so both fallbacks land on the codec path
        // unchanged: no history staged (startup prebuffer, post-reset, post-shed),
        // or a valid codec state — which means the whole Opus path is untouched.
        if self.decoder.plc_ready() || !self.timescale.conceal_frame(&mut self.decoder.decode_buf) {
            self.decoder.decode_plc();
        } else {
            self.decoder.decode_len = OPUS_FRAME_SAMPLES;
            self.log_window.tally.pitch_conceals += 1;
            // Its own field, not `note_splice_quality`, for two reasons. `splice_rms`
            // is documented as describing every splice and only splices — frames the
            // RMS gate judged before cutting audio that was *there*; a concealed
            // frame was judged by nothing and replaces audio that is absent, so
            // folding it in would move that average without any gate having run.
            // And `splice_step` must keep its ceiling: see
            // `TimeScaler::last_conceal_step`.
            if let Some(step) = self.timescale.take_conceal_step() {
                self.log_window.tally.conceal_step_max =
                    self.log_window.tally.conceal_step_max.max(step);
            }
        }

        // Decay the concealment toward — never to — silence, reaching
        // `CONCEAL_FADE_FLOOR` at `conceal_run == 14` (140ms). The threshold is
        // unchanged, so the first three frames of every run still play at unity.
        //
        // The previous schedule (`/4.0`, floored at 0.0) hit **exact digital
        // silence at frame 7**, 60ms in. That was right for the output it was
        // written against — it faded `decode_plc()`, and a codec extrapolated 60ms
        // past its last real frame does sound robotic. What is being faded then
        // changed: on every uncompressed link this is now `conceal_frame` output, a
        // verbatim repetition of the last played pitch period. Muting real audio to
        // zero for the sins of a codec is what the field heard as a dropout — 382
        // frames / 3820ms across 64 rebuffer holds, 57.9% of
        // every 2.4GHz hold, with 39/64 holds running past frame 7. See
        // `CONCEAL_FADE_FLOOR` for the pricing and for why upstream does not fade to
        // silence either.
        //
        // Keyed to `conceal_run`, not `starvation_count`: the latter freezes at
        // `REBUFFER_AFTER` for the whole rebuffer hold (the hold conceals from an
        // early return that never increments it), which pinned this gain at a
        // constant `1.0 - (5-3)/4 = 0.5` for every callback of every hold — a
        // latched statistic, which this module forbids.
        if self.flow.conceal_run > 3 {
            let fade = (1.0 - ((self.flow.conceal_run - 3) as f32 / 12.0)).max(CONCEAL_FADE_FLOOR);
            if fade <= CONCEAL_FADE_FLOOR {
                self.log_window.tally.floor_frames += 1;
            }
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
#[path = "manager_tests.rs"]
mod tests;
