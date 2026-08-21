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
}
