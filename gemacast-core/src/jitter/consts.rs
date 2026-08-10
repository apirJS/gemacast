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
/// Matches NetEQ's `kMaxLag` (60 lags at 4kHz = 15ms, `TEMP/webrtc-neteq/time_stretch.h:85-88`),
/// and bounds the longest period `accelerate` may remove in one splice.
pub(super) const SEARCH_RANGE: usize = 720;

/// How many decoded frames `accelerate` stages into one splice window.
///
/// An OLA removal has to fit inside audio that has not been emitted yet, so a
/// one-frame window (480 sample-frames) could only delete periods of
/// `OLA_LEN..480-OLA_LEN` = **136-375 Hz**. Everything below that — bass, reverb
/// tails, dense mixes — could not clear the 0.9 NCC gate at *any* lag the search
/// was able to test, so a buffer that drifted above `high_limit` on quiet material
/// never shed the surplus (ADB v9: `LOGS/log-ADB-changesv9.txt:179-201` holds
/// 105-116ms for ~80s at rms 0.02-0.07, well inside the masking gate). Two frames
/// widen the reachable period to `OLA_LEN..SEARCH_RANGE` = **67-375 Hz**, matching
/// upstream NetEQ's 66-400 Hz; upstream likewise refuses to accelerate on less than
/// ~30ms of input (`TEMP/webrtc-neteq/accelerate.cc:25-34`).
pub(super) const ACCEL_WINDOW_FRAMES: usize = 2;

/// Decimation factor for the coarse pitch search inside `accelerate`.
/// 48kHz / 12 = 4kHz, the domain NetEQ correlates in
/// (`TEMP/webrtc-neteq/time_stretch.cc:56-60`). A full-rate sweep of the 2-frame
/// window would be ~592 lags x 128 taps ~ 76k FMA on the audio callback thread;
/// coarse-then-refine lands near 12k, below the single-frame search it replaces.
pub(super) const PITCH_DECIMATION: usize = 12;

/// How many coarse correlation peaks are refined at full rate.
/// Decimation blurs adjacent periods together, so refining only the single best
/// bin can miss the true maximum and hand the splice a worse seam than an
/// exhaustive search would have found. Three distinct hills is enough to make the
/// refined result track the exhaustive one on tonal material while keeping the
/// refine budget at 3 x 25 lags.
pub(super) const COARSE_PEAKS: usize = 3;

/// Frames at or below this RMS are treated as silence: excess is shed by dropping
/// whole packets (zero-artifact) rather than by a WSOLA splice.
pub(super) const SILENCE_RMS: f32 = 0.005;

/// Masking threshold for the crude single-pitch-period OLA splice. Normal
/// (non-emergency) accelerate/expand only fire when the frame's RMS is below this,
/// i.e. quiet enough that the edit is psychoacoustically masked. Loud program
/// material (rms ≥ this) is left un-stretched and its overrun tolerated until a
/// quiet moment or, if severe, the ungated emergency drain. Restoring this gate is
/// what keeps aggressive draining artifact-free.
pub(super) const ARTIFACT_MASK_RMS: f32 = 0.08;

/// NCC admission threshold for **preemptive growth** (`expand`), NetEQ's
/// `PreemptiveExpand` (`TEMP/webrtc-neteq/preemptive_expand.cc:40`).
///
/// Upstream's own value is 0.9 and `accelerate` keeps it — the drain is not the
/// constraint, and 128kbps discard was already falling in v19. Growth does not
/// get to keep it, because **0.9 is unreachable on non-tonal program material**.
/// Modelled through this module's real search geometry (`TEMP/v20/thr_curve.py`),
/// acceptance at threshold:
///
/// | threshold | tonal | mixed | percussive |
/// | --- | --- | --- | --- |
/// | 0.90 | 97.0% | **2.0%** | 0.0% |
/// | 0.85 | 99.0% | **59.7%** | 0.0% |
/// | 0.75 | 100% | 77.0% | 1.0% |
/// | 0.50 | 100% | 99.3% | 73.0% |
///
/// The v19 field census agrees: preemptive growth was admitted **11.9%
/// (234/1960)** of the time on 2.4GHz uncompressed and **7.1% (98/1383)** on
/// 128kbps, with `declined_rms_mask` and `declined_recovery` both **0** — nothing
/// else was refusing. Where the refill trigger stood (`filtered < low_limit`),
/// growth was attempted 1457 times and accepted 166.
///
/// **0.85 and not lower.** Worst-case artifact over the admitted set, measured as
/// spectral dip: 0.90 → 32.9% admit at −0.68dB; **0.85 → 41.5% at −1.03dB**;
/// 0.80 → 50.1% at **−5.72dB** — the knee. 0.65 reaches −12.58dB. The step to
/// 0.85 is cheap and the one past it is not, so this constant stops here.
/// Priced against the damage it buys back: +5777ms (uncompressed) / +4738ms
/// (128kbps) of available insertion against 6607 / 1138ms of measured starvation,
/// with the `MIN_EXPAND_INTERVAL` cooldown at 9.1% / 4.4% of slots used.
///
/// The concealment tier (`expand_conceal`) has no threshold at all — see
/// `TimeScaler::expand_conceal`, which is a different upstream operation.
pub(super) const EXPAND_NCC_THRESHOLD: f32 = 0.85;

/// Floor of the concealment fade — the gain a long concealment run decays *to*,
/// never past. −16.5dBFS: quiet, but unambiguously audio.
///
/// **The fade must not reach zero, and the v21 field captures are why.** The
/// schedule it replaces (`1.0 - (conceal_run - 3)/4`, floored at 0.0) reached
/// exact digital silence at `conceal_run == 7` — 60ms into any run. Across the 64
/// rebuffer holds of the five-link v21 round, that muted **382 frames / 3820ms**
/// to zero: 57.9% of every 2.4GHz hold, 51.7% of every 128kbps hold, 77.5% of
/// ADB's. The hold length distribution is what makes that the common case rather
/// than the tail — mean 10.8 frames, median 10, p95 26, and **39/64 (60.9%) run
/// past frame 7**. Six holds in ten were majority-silence. That is the "dropout"
/// the field reported after v21, and it is the only symptom v21 left standing.
///
/// The schedule it replaces was *correct when written*: it faded `decode_plc()`
/// output, and extrapolating a codec 60ms past its last real frame does sound
/// robotic — silence genuinely was the better of those two. v21 changed what is
/// being faded. On every uncompressed link the concealed frame is now
/// [`super::timescale::TimeScaler::conceal_frame`] output — a verbatim repetition
/// of the last played pitch period, source-adjacent at both ends, measured at 565
/// frames on 2.4GHz with zero fallbacks to the silent codec path. That is not
/// degraded prediction; it is audio that really played, replayed. The old schedule
/// muted it for a reason that only ever applied to the output it replaced.
///
/// **0.15 and not lower.** It must sit far enough above zero to read as "the sound
/// got quiet" rather than "the sound stopped", far enough below unity that a long
/// repetition decays instead of sustaining, and comfortably above [`SILENCE_RMS`]
/// (0.005) so that neither the silence fast-forward shed nor the NCC gate's VAD
/// escape changes state because of this gain. Priced over the measured holds
/// (`TEMP/v22/sched.py`): a floor alone removes every zero but reaches the floor at
/// 70ms and sits there (mean gain 0.414/0.461 on the two 2.4GHz links); the 1/12
/// slope alone still zeroes 18.1%/13.9% of hold frames. Together: **zero silent
/// frames on every link**, mean gain 0.327 → 0.576 and 0.384 → 0.633.
///
/// Upstream does not fade to silence either, and the shape of its schedule is
/// commonly misread: `mute_slope` is *signal-derived*
/// (`TEMP/webrtc-neteq/expand.cc:730-763`) and is set to literally **0** — no
/// muting at all — for strongly-voiced material (`slope > 8028`). The fixed
/// constants at `expand.cc:257-266` are gentle floors applied at
/// `consecutive_expands_` 3 and 7 (1.0 → 0.95, 1.0 → 0.90 over 6.25ms each), and
/// upstream runs `kMaxConsecutiveExpands = 200` — **two seconds** — before calling
/// a run excessive. When its `mute_factor` does reach 0 the output is not silence:
/// `expand.cc:295` hands over to `GenerateBackgroundNoise`. A mute schedule ported
/// without that substitute imports upstream's silence without upstream's
/// replacement for it.
///
/// **The risk this constant accepts.** A long run now repeats one pitch period at
/// this gain for its whole tail, where upstream rotates three lags
/// (`expand.cc:844-853`) so consecutive expansions are never identical.
/// `conceal_frame` re-stages its history from its own output, so the pitch search
/// reconverges on the same period — the stuck-note failure mode the old fade
/// avoided by truncating to silence. Bounded three ways: `max_missing_for` resets
/// the stream at 100 frames (1s) on ADB/USB/5GHz and 300 (3s) on 2.4GHz, so no run
/// is unbounded; the measured p95 hold is 26 frames (260ms); and this gain is low
/// enough that a sustained tail is quiet. `TimescaleTally::floor_frames` is the
/// counter that lets that risk be graded from a capture instead of a model.
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
