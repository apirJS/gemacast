//! Shared timing constants and small numeric helpers used across the jitter
//! pipeline actors (stats, target controller, timescaler, flow, orchestrator).

use crate::audio::{OPUS_FRAME_SIZE, OPUS_SAMPLE_RATE};

/// Milliseconds of audio represented by a single Opus frame / packet slot.
pub(super) const MILLIS_PER_FRAME: u32 = (OPUS_FRAME_SIZE as u32 * 1000) / OPUS_SAMPLE_RATE;

/// OLA window length in sample-frames for WSOLA crossfading.
/// 128 frames = 2.67ms at 48kHz — long enough for perceptual transparency.
pub(super) const OLA_LEN: usize = 128;

/// Search range in sample-frames for cross-correlation alignment.
/// 720 frames = 15ms at 48kHz, covering the full human pitch period range.
/// Matches NetEQ's `kMaxLag` (60 lags at 4kHz = 15ms, `time_stretch.h:85`), and
/// bounds the longest period `accelerate` may remove in one splice.
pub(super) const SEARCH_RANGE: usize = 720;

/// How many decoded frames `accelerate` stages into one splice window.
///
/// An OLA removal has to fit inside audio that has not been emitted yet, so a
/// one-frame window (480 sample-frames) could only delete periods of
/// `OLA_LEN..480-OLA_LEN` = 136-375 Hz. Everything below that — bass, reverb
/// tails, dense mixes — could not clear the 0.9 NCC gate at any lag the search
/// could test, so a buffer that drifted above `high_limit` on quiet material
/// never shed the surplus. Two frames widen the reachable period to
/// `OLA_LEN..SEARCH_RANGE` = 67-375 Hz, matching NetEQ's 66-400 Hz; upstream
/// likewise refuses to accelerate on less than ~30ms of input
/// (`accelerate.cc:25`).
pub(super) const ACCEL_WINDOW_FRAMES: usize = 2;

/// Decimation factor for the coarse pitch search inside `accelerate`.
/// 48kHz / 12 = 4kHz, the domain NetEQ correlates in (`time_stretch.cc:56`). A
/// full-rate sweep of the 2-frame window would be ~592 lags x 128 taps ~ 76k FMA
/// on the audio callback thread; coarse-then-refine lands near 12k.
pub(super) const PITCH_DECIMATION: usize = 12;

/// How many coarse correlation peaks are refined at full rate.
/// Decimation blurs adjacent periods together, so refining only the single best
/// bin can miss the true maximum. Three distinct hills makes the refined result
/// track an exhaustive search on tonal material at a budget of 3 x 25 lags.
pub(super) const COARSE_PEAKS: usize = 3;

/// Frames at or below this RMS are treated as silence: excess is shed by dropping
/// whole packets (zero-artifact) rather than by a WSOLA splice.
pub(super) const SILENCE_RMS: f32 = 0.005;

/// Loudness threshold above which a splice is counted as unmasked.
///
/// This is an observation, not a gate: it no longer decides whether a splice is
/// attempted. As a gate it declined 93-96% of 36,455 drain attempts, leaving 243
/// splices — the drain had effectively no actuator on program material. NCC is
/// the quality gate now; this only feeds `tally.loud_splices`.
pub(super) const ARTIFACT_MASK_RMS: f32 = 0.08;

/// NCC admission threshold for preemptive growth (`expand`), NetEQ's
/// `PreemptiveExpand` (`preemptive_expand.cc:40`).
///
/// Upstream uses 0.9 and `accelerate` keeps it — the drain is not the
/// constraint. Growth does not, because 0.9 is unreachable on non-tonal program
/// material. Acceptance modelled through this module's real search geometry:
///
/// | threshold | tonal | mixed | percussive |
/// | --- | --- | --- | --- |
/// | 0.90 | 97.0% | 2.0% | 0.0% |
/// | 0.85 | 99.0% | 59.7% | 0.0% |
/// | 0.75 | 100% | 77.0% | 1.0% |
/// | 0.50 | 100% | 99.3% | 73.0% |
///
/// 0.85 and not lower: worst-case spectral dip over the admitted set is −0.68dB
/// at 0.90, −1.03dB at 0.85, then −5.72dB at 0.80 — the knee. In the field this
/// moved preemptive acceptance from ~12% to ~41%, halving starvation on
/// uncompressed links.
///
/// The concealment tier (`expand_conceal`) has no threshold at all — see
/// `TimeScaler::expand_conceal`, which is a different upstream operation.
pub(super) const EXPAND_NCC_THRESHOLD: f32 = 0.85;

/// Floor of the concealment fade — the gain a long concealment run decays *to*,
/// never past. −16.5dBFS: quiet, but unambiguously audio.
///
/// The fade must not reach zero. The schedule this replaces
/// (`1.0 - (conceal_run - 3)/4`, floored at 0.0) hit exact digital silence at
/// `conceal_run == 7`, 60ms into any run, and measured holds run well past that
/// — mean 10.8 frames, median 10, p95 26, with 61% of holds exceeding frame 7.
/// Six holds in ten were majority-silence, which is what a listener hears as a
/// dropout.
///
/// That schedule was correct when written: it faded `decode_plc()` output, and
/// extrapolating a codec 60ms past its last real frame does sound robotic.
/// What is faded changed. On every uncompressed link the concealed frame is now
/// [`super::timescale::TimeScaler::conceal_frame`] output — a verbatim
/// repetition of the last played pitch period, source-adjacent at both ends.
/// That is not degraded prediction; it is audio that really played, replayed.
///
/// 0.15 and not lower: far enough above zero to read as "the sound got quiet"
/// rather than "the sound stopped", far enough below unity that a long
/// repetition decays instead of sustaining, and comfortably above [`SILENCE_RMS`]
/// (0.005) so neither the silence fast-forward shed nor the NCC gate's VAD
/// escape changes state because of this gain. Both halves are load-bearing: a
/// floor alone bottoms out at 70ms and sits there; the 1/12 slope alone still
/// zeroes ~15% of hold frames. Together, zero silent frames on every link.
///
/// Upstream does not fade to silence either, and its schedule is commonly
/// misread: `mute_slope` is signal-derived (`expand.cc:730`) and is literally 0
/// — no muting — for strongly-voiced material. The fixed constants at
/// `expand.cc:257` are gentle floors applied at `consecutive_expands_` 3 and 7
/// (1.0 → 0.95, 1.0 → 0.90 over 6.25ms each), and `kMaxConsecutiveExpands = 200`
/// is two seconds before a run is called excessive. When `mute_factor` does
/// reach 0 the output is not silence: `expand.cc:295` hands over to
/// `GenerateBackgroundNoise`. Porting the mute schedule without that substitute
/// imports upstream's silence without upstream's replacement for it.
///
/// The risk this accepts: a long run repeats one pitch period at this gain for
/// its whole tail, where upstream rotates three lags (`expand.cc:844`) so
/// consecutive expansions are never identical. `conceal_frame` re-stages history
/// from its own output, so the pitch search reconverges on the same period — a
/// stuck note. Bounded three ways: `max_missing_for` resets the stream at 100
/// frames (1s) on ADB/USB/5GHz and 300 (3s) on 2.4GHz; the measured p95 hold is
/// 260ms; and this gain is low enough that a sustained tail is quiet.
/// `TimescaleTally::floor_frames` counts frames emitted at the floor so the risk
/// can be graded from a capture rather than a model.
pub(super) const CONCEAL_FADE_FLOOR: f32 = 0.15;

/// Convert milliseconds to frames using ceiling division.
/// Prevents truncation to 0 for sub-frame values (e.g. 2ms / 5ms = 1 frame, not 0).
pub(super) fn ms_to_frames_ceil(ms: u32) -> u32 {
    ms.div_ceil(MILLIS_PER_FRAME)
}

/// Linear interpolation between `a` and `b` by fraction `t`.
pub(super) fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}
