//! Platform-neutral sample decoding, channel folding, and format validation.
//!
//! These are pure functions over `f32` and byte slices with no platform dependency.
//! They live here rather than beside the WASAPI adapter that first needed them for
//! one reason: `wasapi_common.rs` is `#![cfg(target_os = "windows")]`, so anything
//! inside it **cannot compile on three of the four CI backend legs** and therefore
//! could not be tested at all. Both functions below had zero test coverage while they
//! were in there, which is how the two downmix defects recorded on
//! [`downmix_to_stereo`] survived unnoticed.
//!
//! `wasapi_common` re-exports these, so Windows call sites are unchanged.
//!
//! The arithmetic here is a **verbatim move**. Nothing in this module changes what a
//! correctly-behaving device produces; the only new behaviour is the `channels == 0`
//! guard in [`downmix_to_stereo`], which replaces an integer division by zero.

use crate::audio::{OPUS_CHANNELS, OPUS_SAMPLE_RATE};
use crate::domain::error::{AudioError, GemaCastError};
use crate::ports::capture::CaptureCounters;

/// Validate a negotiated capture format against the pipeline contract.
///
/// The contract is 48 kHz stereo (see [`crate::ports::capture`]). Call this **once at
/// adapter construction**, after format negotiation resolves — never per buffer.
///
/// A backend that can resample or downmix into the contract should do so and not call
/// this at all. The error exists for formats that cannot be adapted — a planar
/// layout, a sample type the decoder does not handle — and for catching a degenerate
/// descriptor such as zero channels before it reaches arithmetic that divides by it.
///
/// # Errors
///
/// [`AudioError::UnsupportedCaptureFormat`] if the rate is not 48 kHz or the channel
/// count is not 2.
pub fn validate_capture_format(rate: u32, channels: usize) -> Result<(), GemaCastError> {
    if rate == OPUS_SAMPLE_RATE && channels == OPUS_CHANNELS as usize {
        Ok(())
    } else {
        Err(AudioError::UnsupportedCaptureFormat { rate, channels }.into())
    }
}

/// Fold interleaved multi-channel audio down to interleaved stereo.
///
/// `output` is cleared and refilled with `input.len() / channels` stereo pairs. A
/// trailing partial frame is discarded.
///
/// - **1 channel** — duplicated into both output channels at unity gain.
/// - **2 channels** — copied verbatim at unity gain. This is the level every other
///   branch has to match.
/// - **more than 2** — folded by SMPTE index order: index 2 treated as front centre at
///   −3 dB, 3 as LFE at 0.3, 4/5 as rear at −3 dB, 6/7 as side at −3 dB, then divided
///   by the sum of the contributing coefficients.
/// - **0 channels** — produces empty output. This is a malformed platform descriptor,
///   not a valid stream; guarding it is what stops the `input.len() / channels` below
///   from panicking on integer division by zero.
pub fn downmix_to_stereo(input: &[f32], channels: usize, output: &mut Vec<f32>) {
    output.clear();

    // A zero channel count reaches here from an unvalidated `WAVEFORMATEX`. Guarding
    // it is the whole reason this branch exists — `input.len() / channels` below is an
    // integer division and panics rather than producing garbage.
    if channels == 0 {
        return;
    }

    let frames = input.len() / channels;

    match channels {
        1 => {
            output.reserve(frames * 2);
            for &s in input.iter().take(frames) {
                output.push(s);
                output.push(s);
            }
        }
        2 => {
            output.reserve(input.len());
            output.extend_from_slice(input);
        }
        _ => {
            output.reserve(frames * 2);
            for frame in input.chunks_exact(channels) {
                // FL (0), FR (1), C (2)
                let center = frame.get(2).copied().unwrap_or(0.0) * 0.707;

                let mut left = frame[0] + center;
                let mut right = frame[1] + center;

                // LFE (3)
                if channels >= 4 {
                    let lfe = frame[3] * 0.3;
                    left += lfe;
                    right += lfe;
                }

                // RL (4), RR (5)
                if channels >= 6 {
                    left += frame[4] * 0.707;
                    right += frame[5] * 0.707;
                }

                // SL (6), SR (7)
                if channels >= 8 {
                    left += frame[6] * 0.707;
                    right += frame[7] * 0.707;
                }

                // Prevent clipping
                let norm = if channels >= 8 {
                    1.0 + 0.707 + 0.3 + 0.707 + 0.707
                } else if channels >= 6 {
                    1.0 + 0.707 + 0.3 + 0.707
                } else if channels >= 4 {
                    1.0 + 0.707 + 0.3
                } else {
                    1.0 + 0.707
                };

                output.push(left / norm);
                output.push(right / norm);
            }
        }
    }
}

/// Convert one packed little-endian PCM sample to `f32`, nominally −1.0..=1.0.
///
/// `bytes` must be at least `bytes_per_sample` long; extra bytes are ignored. Returns
/// `None` for a width this does not handle, so a caller can report an unrecognised
/// format rather than emitting silence indistinguishable from real silence.
///
/// This is the reference definition of the pipeline's sample arithmetic, extracted so
/// it can be tested on every CI leg. The 24-bit case places the sample in the **high**
/// three bytes of an `i32` and scales by the full 32-bit range, which keeps one scale
/// factor across all widths — a value at half of full scale decodes to the same `f32`
/// whether it arrived as 16-, 24-, or 32-bit.
///
/// Endianness is little-endian throughout. Every target this ships on is
/// little-endian, and both producers are explicit about it: PipeWire negotiates
/// `F32LE`, and WASAPI delivers host-endian, which coincides.
pub fn pcm_sample_to_f32(bytes: &[u8], bytes_per_sample: usize, is_float: bool) -> Option<f32> {
    /// Scale for a signed sample occupying the full width of an `i32`.
    const I32_SCALE: f32 = 2_147_483_648.0;

    if bytes.len() < bytes_per_sample {
        return None;
    }

    match (is_float, bytes_per_sample) {
        (true, 4) => Some(f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])),
        (false, 2) => Some(i16::from_le_bytes([bytes[0], bytes[1]]) as f32 / 32768.0),
        // 24-bit packed: shifted into the high three bytes so the divisor matches the
        // 32-bit case rather than needing one of its own.
        (false, 3) => {
            let val = i32::from_le_bytes([0, bytes[0], bytes[1], bytes[2]]);
            Some(val as f32 / I32_SCALE)
        }
        (false, 4) => {
            let val = i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
            Some(val as f32 / I32_SCALE)
        }
        _ => None,
    }
}

/// Reinterpret a platform byte buffer as `f32`, counting any misaligned head bytes.
///
/// [`slice::align_to`] finds the largest `f32`-aligned run inside `bytes` and hands back
/// the bytes before it (the prefix) and after it (the suffix) separately. A non-empty
/// prefix means the buffer did not start on a 4-byte boundary — Core Audio buffers
/// effectively always do, so a non-zero `unaligned_prefix_bytes` is a tripwire that the
/// assumption broke, not an expected event. The trailing suffix (1–3 bytes that cannot
/// form a whole `f32`) is dropped; SCK delivers whole samples, so it is likewise a
/// can't-happen remainder rather than something to carry.
///
/// # Safety of the cast
///
/// `align_to::<f32>` is `unsafe` because the middle elements must be valid `f32` values.
/// Every 32-bit pattern *is* a valid `f32` (unlike `bool` or `char`, `f32` has no
/// invalid bit patterns — NaNs included), so reinterpreting arbitrary bytes as `f32` is
/// always sound.
fn bytes_as_f32<'a>(bytes: &'a [u8], counters: &CaptureCounters) -> &'a [f32] {
    // SAFETY: see the doc above — all f32 bit patterns are valid.
    let (prefix, samples, _suffix) = unsafe { bytes.align_to::<f32>() };
    if !prefix.is_empty() {
        CaptureCounters::add(&counters.unaligned_prefix_bytes, prefix.len() as u64);
    }
    samples
}

/// Fold one ScreenCaptureKit audio callback's raw buffers into the pipeline's
/// **interleaved-stereo `f32`** contract (see [`crate::ports::capture`]).
///
/// This is the byte-level heart of the macOS capture path, deliberately free of any
/// ScreenCaptureKit type: it takes the already-extracted byte payload of each
/// `AudioBuffer` in the callback's `AudioBufferList` and nothing else, so the
/// planar-vs-interleaved logic — the part that was wrong and unfalsifiable while it
/// lived in the `#[cfg(target_os = "macos")]` handler — can be unit-tested on every CI
/// leg with synthetic buffers. The macOS glue in `sck_common` becomes a thin extractor
/// that calls this.
///
/// The caller has already established the stream is stereo (obligation 3:
/// [`validate_capture_format`] at construction), so the buffer **count** alone selects
/// the layout — the shape Core Audio actually chose, which our `SCStreamConfiguration`
/// requests but cannot force, so OBS reads it back rather than assuming and so do we:
///
/// - **2 buffers → planar float**, one channel per buffer (`FLOAT_PLANAR`, what SCK
///   delivers in practice). Interleaved `[L, R, L, R, …]` up to the shorter channel.
///   A length imbalance leaves orphan samples that can never be paired — a malformed
///   buffer a real stream never produces — so they are dropped and counted in
///   `truncated_samples`. This path always emits whole pairs and never sets `carry`.
/// - **1 buffer → already-interleaved stereo**, passed through after prepending any
///   carried sample (below).
/// - **anything else** (0, or >2) → an unrecognised layout; counted in
///   `unknown_format_buffers` and produces no samples, never silence-as-sound.
///
/// `carry` upholds obligation 1 — *every push carries an even sample count*. An
/// interleaved buffer with an odd `f32` count ends mid-pair on a lone left sample; that
/// sample is held in `carry` and prepended to the next callback (where it meets its
/// right partner) rather than dropped or pushed, either of which would swap L/R for the
/// rest of the session. Each hold-over is counted in `truncated_samples`, whose doc
/// names exactly this case. In a well-formed SCK stream every buffer is a whole number
/// of frames, so `carry` stays `None` — it is the boundary-split safety net the port
/// contract mandates, not an expected path.
///
/// `out` is cleared and refilled (matching [`downmix_to_stereo`]); `carry` and
/// `counters` are updated in place.
pub fn sck_buffers_to_interleaved_stereo(
    buffers: &[&[u8]],
    carry: &mut Option<f32>,
    out: &mut Vec<f32>,
    counters: &CaptureCounters,
) {
    out.clear();

    match buffers {
        // Interleaved stereo: one buffer already in `[L, R, L, R, …]` order. Splice the
        // carried odd sample onto the front, then hold a new odd sample back so this
        // push — and thus every push — is a whole number of stereo pairs.
        [interleaved] => {
            if let Some(prev) = carry.take() {
                out.push(prev);
            }
            out.extend_from_slice(bytes_as_f32(interleaved, counters));
            if out.len() % 2 == 1 {
                *carry = out.pop();
                CaptureCounters::add(&counters.truncated_samples, 1);
            }
        }
        // Planar float: two buffers, left channel then right. Interleave up to the
        // shorter one; a real stream delivers equal lengths, so any remainder is a
        // malformed buffer whose orphan samples cannot be paired.
        [left_bytes, right_bytes] => {
            let left = bytes_as_f32(left_bytes, counters);
            let right = bytes_as_f32(right_bytes, counters);
            let pairs = left.len().min(right.len());

            let orphans = (left.len().max(right.len()) - pairs) as u64;
            if orphans > 0 {
                CaptureCounters::add(&counters.truncated_samples, orphans);
            }

            out.reserve(pairs * 2);
            for i in 0..pairs {
                out.push(left[i]);
                out.push(right[i]);
            }
        }
        // 0 buffers, or more than 2: not a stereo layout we recognise. Emit nothing and
        // trip the counter rather than guessing at a channel arrangement.
        _ => {
            CaptureCounters::add(&counters.unknown_format_buffers, 1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod format_validation {
        use super::*;

        #[test]
        fn should_accept_the_pipeline_contract() {
            assert!(validate_capture_format(OPUS_SAMPLE_RATE, 2).is_ok());
        }

        #[test]
        fn should_reject_a_non_48k_rate() {
            assert!(validate_capture_format(44_100, 2).is_err());
        }

        #[test]
        fn should_reject_a_non_stereo_channel_count() {
            assert!(validate_capture_format(OPUS_SAMPLE_RATE, 6).is_err());
            assert!(validate_capture_format(OPUS_SAMPLE_RATE, 1).is_err());
        }

        // Zero channels is the descriptor that reaches an integer division downstream,
        // so it must be rejected rather than treated as "some channel count".
        #[test]
        fn should_reject_zero_channels() {
            assert!(validate_capture_format(OPUS_SAMPLE_RATE, 0).is_err());
        }

        #[test]
        fn should_name_the_offending_values_in_the_error() {
            let err = validate_capture_format(44_100, 6).unwrap_err().to_string();
            assert!(
                err.contains("44100") && err.contains('6'),
                "error should carry the rejected format, got: {err}"
            );
        }
    }

    mod downmix {
        use super::*;

        #[test]
        fn should_duplicate_mono_into_both_channels() {
            let mut out = Vec::new();
            downmix_to_stereo(&[0.5, -0.25], 1, &mut out);
            assert_eq!(out, vec![0.5, 0.5, -0.25, -0.25]);
        }

        #[test]
        fn should_pass_stereo_through_at_unity_gain() {
            let mut out = Vec::new();
            let input = [0.1, -0.2, 0.3, -0.4];
            downmix_to_stereo(&input, 2, &mut out);
            assert_eq!(out, input.to_vec());
        }

        // The crash this module's move fixes: the old first statement was
        // `input.len() / channels`, which panics on integer division by zero. A zero
        // channel count is reachable because `parse_mix_format` does not reject it.
        #[test]
        fn should_not_panic_on_zero_channels() {
            let mut out = Vec::new();
            downmix_to_stereo(&[0.1, 0.2], 0, &mut out);
            assert!(out.is_empty());
        }

        #[test]
        fn should_clear_previous_contents_before_writing() {
            let mut out = vec![9.0; 16];
            downmix_to_stereo(&[0.5, 0.5], 2, &mut out);
            assert_eq!(out, vec![0.5, 0.5]);
        }

        #[test]
        fn should_discard_a_trailing_partial_frame() {
            let mut out = Vec::new();
            // Seven samples of a 6-channel stream: one whole frame plus a stray.
            downmix_to_stereo(&[0.5, 0.5, 0.0, 0.0, 0.0, 0.0, 0.9], 6, &mut out);
            assert_eq!(out.len(), 2, "one complete frame in, one stereo pair out");
        }

        #[test]
        fn should_emit_nothing_for_an_empty_input() {
            let mut out = vec![1.0; 4];
            downmix_to_stereo(&[], 6, &mut out);
            assert!(out.is_empty());
        }

        #[test]
        fn should_split_the_centre_channel_evenly_between_both_sides() {
            let mut out = Vec::new();
            downmix_to_stereo(&[0.0, 0.0, 1.0, 0.0, 0.0, 0.0], 6, &mut out);
            assert_eq!(
                out[0], out[1],
                "centre must arrive equally in both channels"
            );
            assert!(out[0] > 0.0, "centre must arrive at all, got {}", out[0]);
        }

        #[test]
        fn should_mix_the_lfe_channel_into_both_sides() {
            let mut out = Vec::new();
            downmix_to_stereo(&[0.0, 0.0, 0.0, 1.0], 4, &mut out);
            assert_eq!(out[0], out[1], "LFE is non-directional");
            assert!(out[0] > 0.0);
        }

        #[test]
        fn should_keep_the_rear_pair_on_its_own_side() {
            let mut out = Vec::new();
            // 5.1 with content only in rear-left (index 4).
            downmix_to_stereo(&[0.0, 0.0, 0.0, 0.0, 1.0, 0.0], 6, &mut out);
            assert!(out[0] > 0.0, "rear-left should reach the left output");
            assert_eq!(out[1], 0.0, "rear-left must not leak into the right output");
        }

        // Pins defect 2 from the module docs at its measured value, so behaviour commit
        // 3 has a reference to change *from* and this test fails loudly when it does.
        // Not an endorsement — 0.368 is the bug.
        #[test]
        fn should_currently_attenuate_multichannel_relative_to_stereo() {
            let mut stereo = Vec::new();
            downmix_to_stereo(&[1.0, 0.0], 2, &mut stereo);

            let mut surround = Vec::new();
            // Identical front-left content, silence in every other 5.1 channel.
            downmix_to_stereo(&[1.0, 0.0, 0.0, 0.0, 0.0, 0.0], 6, &mut surround);

            assert_eq!(stereo[0], 1.0, "stereo is the unity reference");
            let ratio = surround[0] / stereo[0];
            assert!(
                (ratio - 1.0 / 2.714).abs() < 0.001,
                "5.1 front-left currently folds to {ratio} of the stereo level \
                 (1/2.714 = 0.368, i.e. -8.7 dB). If this assertion is failing, the \
                 normalization changed — that is behaviour commit 3, and it needs a \
                 real 5.1 machine to verify."
            );
        }

        // Pins defect 1: a quad stream's rear-left is folded as if it were the centre
        // channel, so it appears in both outputs. Behaviour commit 3 reads
        // `dwChannelMask` and this becomes a left-only signal.
        #[test]
        fn should_currently_misroute_quad_rear_channels_as_centre_and_lfe() {
            let mut out = Vec::new();
            // FL FR BL BR, content only in back-left (index 2).
            downmix_to_stereo(&[0.0, 0.0, 1.0, 0.0], 4, &mut out);
            assert_eq!(
                out[0], out[1],
                "back-left is currently treated as centre, so it lands in both \
                 channels. If this fails, the channel-mask fold landed — that is \
                 behaviour commit 3."
            );
            assert!(out[0] > 0.0);
        }

        #[test]
        fn should_handle_every_channel_count_from_one_to_eight_without_panicking() {
            for channels in 1..=8usize {
                let input = vec![0.25f32; channels * 3];
                let mut out = Vec::new();
                downmix_to_stereo(&input, channels, &mut out);
                assert_eq!(
                    out.len(),
                    6,
                    "{channels} channels x 3 frames should yield 3 stereo pairs"
                );
                assert!(
                    out.iter().all(|s| s.is_finite()),
                    "{channels} channels produced a non-finite sample: {out:?}"
                );
            }
        }
    }

    mod sample_decode {
        use super::*;

        #[test]
        fn should_round_trip_f32_samples_bit_exactly() {
            let bytes = 0.375f32.to_le_bytes();
            assert_eq!(pcm_sample_to_f32(&bytes, 4, true), Some(0.375));
        }

        #[test]
        fn should_scale_i16_full_scale_to_unity() {
            assert_eq!(
                pcm_sample_to_f32(&i16::MIN.to_le_bytes(), 2, false),
                Some(-1.0)
            );
            let max = pcm_sample_to_f32(&i16::MAX.to_le_bytes(), 2, false).unwrap();
            assert!((max - 1.0).abs() < 0.0001, "got {max}");
        }

        #[test]
        fn should_scale_i32_full_scale_to_unity() {
            assert_eq!(
                pcm_sample_to_f32(&i32::MIN.to_le_bytes(), 4, false),
                Some(-1.0)
            );
        }

        // 24-bit sits in the high three bytes, so full-scale negative is 0x800000
        // stored little-endian, and it must scale to -1.0 like every other width.
        #[test]
        fn should_scale_24_bit_on_the_same_scale_as_other_widths() {
            assert_eq!(pcm_sample_to_f32(&[0x00, 0x00, 0x80], 3, false), Some(-1.0));
            assert_eq!(pcm_sample_to_f32(&[0x00, 0x00, 0x00], 3, false), Some(0.0));
        }

        #[test]
        fn should_agree_across_widths_for_the_same_nominal_level() {
            let half_16 = pcm_sample_to_f32(&(i16::MAX / 2).to_le_bytes(), 2, false).unwrap();
            let half_24 = pcm_sample_to_f32(&[0x00, 0x00, 0x40], 3, false).unwrap();
            let half_32 = pcm_sample_to_f32(&(i32::MAX / 2).to_le_bytes(), 4, false).unwrap();
            assert!(
                (half_16 - half_24).abs() < 0.001 && (half_24 - half_32).abs() < 0.001,
                "16-bit {half_16}, 24-bit {half_24}, 32-bit {half_32} should agree"
            );
        }

        // The caller must be able to tell "unhandled width" from "silence", which the
        // old code could not do — it emitted zeros for both.
        #[test]
        fn should_report_an_unhandled_width_rather_than_returning_silence() {
            assert_eq!(pcm_sample_to_f32(&[0; 8], 8, true), None);
            assert_eq!(pcm_sample_to_f32(&[0; 4], 1, false), None);
            // f32 is the only float width; a 2-byte float is not a format we decode.
            assert_eq!(pcm_sample_to_f32(&[0; 4], 2, true), None);
        }

        #[test]
        fn should_reject_a_slice_shorter_than_the_sample_width() {
            assert_eq!(pcm_sample_to_f32(&[0x01, 0x02], 4, false), None);
            assert_eq!(pcm_sample_to_f32(&[], 2, false), None);
        }

        #[test]
        fn should_ignore_bytes_past_the_sample_width() {
            let padded = [0x00, 0x00, 0x80, 0xFF, 0xFF];
            assert_eq!(pcm_sample_to_f32(&padded, 3, false), Some(-1.0));
        }
    }

    /// The ScreenCaptureKit planar/interleaved fold and its odd-sample carry.
    ///
    /// This is the whole reason the byte-math was pulled out of the macOS-only handler:
    /// none of these ran on any CI leg before, and the interleaved-vs-planar bug they
    /// pin was invisible without a Mac. The layout is selected by buffer count, so every
    /// case here is reachable with synthetic byte slices and no ScreenCaptureKit type.
    mod sck_interleave {
        use super::*;
        use std::sync::atomic::Ordering;

        /// A 4-byte-aligned view of `backing`'s bytes.
        ///
        /// Test inputs must start on an `f32` boundary, or the `align_to` inside the
        /// function under test would skip a *nondeterministic* prefix and read the
        /// samples from a shifted offset — a raw `Vec<u8>` guarantees only 1-byte
        /// alignment. A `Vec<f32>` is `≥4`-aligned, and `align_to::<u8>` on it has an
        /// empty prefix (`u8` divides every alignment), so the returned slice starts at
        /// that aligned address. The caller keeps `backing` alive while borrowing.
        fn as_bytes(backing: &[f32]) -> &[u8] {
            let (prefix, bytes, _suffix) = unsafe { backing.align_to::<u8>() };
            debug_assert!(prefix.is_empty(), "an f32 slice viewed as u8 has no prefix");
            bytes
        }

        // The core discriminator against the code this replaces. The old extractor did
        // `audio_list.iter().next()` — the *first* buffer only, which in a planar stream
        // is the left channel — and reinterpreted it as interleaved. On this input that
        // produced `[10.0, 20.0]` (left, alone). A naive "concatenate both buffers"
        // fix would give `[10, 20, 30, 40]`. Only interleaving both channels gives the
        // pipeline's `[L, R, L, R]`, so this assertion fails against either wrong shape.
        #[test]
        fn should_interleave_two_planar_buffers_as_left_right_pairs() {
            let left = vec![10.0f32, 20.0];
            let right = vec![30.0f32, 40.0];

            let counters = CaptureCounters::default();
            let mut carry = None;
            let mut out = Vec::new();
            sck_buffers_to_interleaved_stereo(
                &[as_bytes(&left), as_bytes(&right)],
                &mut carry,
                &mut out,
                &counters,
            );

            assert_eq!(out, vec![10.0, 30.0, 20.0, 40.0]);
            assert!(carry.is_none(), "the planar path never sets carry");
            assert!(
                counters.all_clear(),
                "a balanced planar buffer is clean: {:?}",
                counters.snapshot()
            );
        }

        #[test]
        fn should_pass_a_single_interleaved_buffer_through_unchanged() {
            let buf = vec![0.5f32, -0.25, 0.75, -0.125];

            let counters = CaptureCounters::default();
            let mut carry = None;
            let mut out = Vec::new();
            sck_buffers_to_interleaved_stereo(&[as_bytes(&buf)], &mut carry, &mut out, &counters);

            assert_eq!(out, vec![0.5, -0.25, 0.75, -0.125]);
            assert!(carry.is_none());
            assert!(counters.all_clear(), "{:?}", counters.snapshot());
        }

        // Obligation 1: an odd interleaved count ends mid-pair. The lone trailing sample
        // must be held in `carry`, not emitted, and the hold counted.
        #[test]
        fn should_hold_an_odd_interleaved_sample_in_carry() {
            let buf = vec![1.0f32, 2.0, 3.0]; // one and a half pairs

            let counters = CaptureCounters::default();
            let mut carry = None;
            let mut out = Vec::new();
            sck_buffers_to_interleaved_stereo(&[as_bytes(&buf)], &mut carry, &mut out, &counters);

            assert_eq!(out, vec![1.0, 2.0], "only the whole pair is emitted");
            assert_eq!(carry, Some(3.0), "the orphan left sample is held over");
            assert_eq!(out.len() % 2, 0, "every push must be an even count");
            assert_eq!(counters.truncated_samples.load(Ordering::Relaxed), 1);
        }

        // The other half of obligation 1: the carried sample rejoins the stream on the
        // next callback and pairs with that callback's first sample. Two odd buffers in
        // a row must reassemble as `[L0,R0]` then `[L1,R1]`, never lose or duplicate a
        // sample, and never leave the stream one slot out of phase.
        #[test]
        fn should_prepend_a_carried_sample_onto_the_next_callback() {
            let counters = CaptureCounters::default();
            let mut carry = None;
            let mut out = Vec::new();

            // Callback 1: [L0, R0, L1] — emit the pair, hold L1.
            let first = vec![1.0f32, 2.0, 3.0];
            sck_buffers_to_interleaved_stereo(&[as_bytes(&first)], &mut carry, &mut out, &counters);
            assert_eq!(out, vec![1.0, 2.0]);
            assert_eq!(carry, Some(3.0));

            // Callback 2: [R1, L2] — L1 (carried) meets R1, then L2 is held.
            let second = vec![4.0f32, 5.0];
            sck_buffers_to_interleaved_stereo(
                &[as_bytes(&second)],
                &mut carry,
                &mut out,
                &counters,
            );
            assert_eq!(
                out,
                vec![3.0, 4.0],
                "carried L1 pairs with this buffer's R1"
            );
            assert_eq!(carry, Some(5.0), "L2 is now the held-over sample");
            assert_eq!(
                counters.truncated_samples.load(Ordering::Relaxed),
                2,
                "one hold-over per odd buffer"
            );
        }

        // An even buffer that follows a carry consumes the carry and holds nothing: the
        // carried sample plus an odd number of new samples makes an even total.
        #[test]
        fn should_drain_the_carry_when_the_combined_count_is_even() {
            let counters = CaptureCounters::default();
            let mut carry = Some(1.0);
            let mut out = Vec::new();

            // 3 new samples + 1 carried = 4 = two whole pairs, nothing left over.
            let buf = vec![2.0f32, 3.0, 4.0];
            sck_buffers_to_interleaved_stereo(&[as_bytes(&buf)], &mut carry, &mut out, &counters);

            assert_eq!(out, vec![1.0, 2.0, 3.0, 4.0]);
            assert!(carry.is_none(), "an even combined count leaves no carry");
            assert!(counters.all_clear(), "{:?}", counters.snapshot());
        }

        // A malformed planar buffer whose channels differ in length: the orphan samples
        // of the longer channel cannot be paired and are dropped, counted as truncated.
        #[test]
        fn should_drop_and_count_orphans_on_a_planar_length_mismatch() {
            let left = vec![1.0f32, 2.0, 3.0];
            let right = vec![4.0f32, 5.0]; // one short

            let counters = CaptureCounters::default();
            let mut carry = None;
            let mut out = Vec::new();
            sck_buffers_to_interleaved_stereo(
                &[as_bytes(&left), as_bytes(&right)],
                &mut carry,
                &mut out,
                &counters,
            );

            assert_eq!(
                out,
                vec![1.0, 4.0, 2.0, 5.0],
                "two pairs, up to the shorter side"
            );
            assert!(carry.is_none());
            assert_eq!(
                counters.truncated_samples.load(Ordering::Relaxed),
                1,
                "the unpaired third left sample"
            );
        }

        #[test]
        fn should_count_an_empty_buffer_list_as_an_unknown_layout() {
            let counters = CaptureCounters::default();
            let mut carry = None;
            let mut out = vec![9.0]; // must be cleared even on the reject path
            let empty: &[&[u8]] = &[];
            sck_buffers_to_interleaved_stereo(empty, &mut carry, &mut out, &counters);

            assert!(out.is_empty());
            assert_eq!(counters.unknown_format_buffers.load(Ordering::Relaxed), 1);
        }

        #[test]
        fn should_count_more_than_two_buffers_as_an_unknown_layout() {
            let a = vec![1.0f32];
            let b = vec![2.0f32];
            let c = vec![3.0f32];

            let counters = CaptureCounters::default();
            let mut carry = None;
            let mut out = Vec::new();
            sck_buffers_to_interleaved_stereo(
                &[as_bytes(&a), as_bytes(&b), as_bytes(&c)],
                &mut carry,
                &mut out,
                &counters,
            );

            assert!(out.is_empty(), "a layout we do not recognise emits nothing");
            assert_eq!(counters.unknown_format_buffers.load(Ordering::Relaxed), 1);
            assert_eq!(
                counters.truncated_samples.load(Ordering::Relaxed),
                0,
                "an unknown layout is not a truncation"
            );
        }

        // The `unaligned_prefix_bytes` tripwire. Core Audio buffers start aligned, so
        // this path effectively never fires in the field — the count exists so that
        // belief is falsifiable. Here we force a 2-byte misalignment deterministically.
        #[test]
        fn should_count_the_prefix_bytes_it_skips_to_reach_alignment() {
            // A literal f32 array is ≥4-aligned (static-promoted here); its byte view
            // starts at that aligned address.
            let backing: &[f32] = &[1.0f32, 2.0, 3.0];
            let (prefix, all_bytes, _) = unsafe { backing.align_to::<u8>() };
            assert!(
                prefix.is_empty(),
                "byte view starts at the aligned f32 address"
            );

            // Offset by 2 bytes: the next f32 boundary is 2 bytes ahead, so align_to
            // inside the function must skip exactly 2 and then read 2.0 and 3.0.
            let misaligned = &all_bytes[2..];

            let counters = CaptureCounters::default();
            let mut carry = None;
            let mut out = Vec::new();
            sck_buffers_to_interleaved_stereo(&[misaligned], &mut carry, &mut out, &counters);

            assert_eq!(
                out,
                vec![2.0, 3.0],
                "the aligned f32 run after the skipped head"
            );
            assert_eq!(
                counters.unaligned_prefix_bytes.load(Ordering::Relaxed),
                2,
                "the two bytes skipped to reach f32 alignment"
            );
        }

        // A trailing 1–3 bytes that cannot complete an f32 are dropped by align_to's
        // suffix. SCK delivers whole samples, so this is a can't-happen remainder; the
        // test pins that it is silently ignored rather than read past or panicked on.
        #[test]
        fn should_ignore_a_trailing_partial_f32() {
            let backing = vec![7.0f32, 8.0];
            let full = as_bytes(&backing);
            // 9 of 8 bytes: two whole f32s plus one stray byte.
            let mut with_stray = full[..8].to_vec();
            with_stray.push(0xAB);

            // Re-align: the truncated copy is a fresh Vec<u8> (1-aligned). Rather than
            // fight its alignment, assert only on the sample count and evenness, which
            // hold regardless of any prefix skip.
            let counters = CaptureCounters::default();
            let mut carry = None;
            let mut out = Vec::new();
            sck_buffers_to_interleaved_stereo(&[&with_stray], &mut carry, &mut out, &counters);

            assert_eq!(out.len() % 2, 0, "output stays an even sample count");
            assert!(out.len() <= 2, "the stray byte cannot form a third sample");
        }
    }
}
