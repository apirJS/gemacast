use crate::audio::{FORMAT_OPUS, FORMAT_SILENCE, FORMAT_UNCOMPRESSED, OPUS_FRAME_SAMPLES};
use crate::domain::error::{AudioError, CodecDirection, GemaCastError};
use crate::domain::types::AudioBitrate;
use opus::Encoder;

#[derive(Debug)]
pub enum EncodeResult {
    Encoded,
}

/// RMS below which a frame is transmitted as a one-byte silence marker instead of a
/// payload.
///
/// Unrelated to the jitter buffer's `SILENCE_RMS` (0.005), which decides whether the
/// *receiver* may fast-forward through silence. This one is a send-side bandwidth
/// optimisation and is deliberately far lower: anything above it is real program
/// material that must be encoded, and mistaking quiet music for silence here would
/// drop it from the stream entirely.
const SILENCE_RMS_THRESHOLD: f32 = 0.0001;

/// Encode one 10 ms stereo frame into `packet_buf` as a wire packet.
///
/// `frame` must be exactly [`OPUS_FRAME_SAMPLES`] interleaved `f32` values — 480
/// sample-pairs at 48 kHz. See the [capture format
/// contract](crate::ports::capture#the-capture-format-contract); this function is the
/// last place that invariant can still be checked before it reaches the wire.
///
/// # Errors
///
/// * [`AudioError::InvalidFrameLength`] if `frame` is not `OPUS_FRAME_SAMPLES` long.
/// * [`AudioError::CaptureInstanceFailed`] if a compressed bitrate was requested with
///   no encoder.
/// * [`AudioError::OpusCodecFailed`] if Opus encoding fails.
pub fn encode_frame(
    frame: &[f32],
    encoder: Option<&mut Encoder>,
    bitrate: AudioBitrate,
    seq_num: u64,
    opus_output: &mut [u8],
    packet_buf: &mut Vec<u8>,
) -> Result<EncodeResult, GemaCastError> {
    // Checked before anything reads `frame`, which is what makes the raw-pointer cast
    // in the uncompressed branch sound unconditionally and keeps the RMS divisor
    // non-zero.
    if frame.len() != OPUS_FRAME_SAMPLES {
        return Err(AudioError::InvalidFrameLength {
            got: frame.len(),
            expected: OPUS_FRAME_SAMPLES,
        }
        .into());
    }

    let mut sum_sq = 0.0f32;
    for sample in frame {
        sum_sq += sample * sample;
    }
    let rms = (sum_sq / frame.len() as f32).sqrt();

    let is_silence = rms < SILENCE_RMS_THRESHOLD;
    let is_uncompressed = bitrate == AudioBitrate::Uncompressed;

    let format_flag = if is_silence {
        FORMAT_SILENCE
    } else if is_uncompressed {
        FORMAT_UNCOMPRESSED
    } else {
        FORMAT_OPUS
    };

    let payload_bytes: &[u8] = if is_silence {
        &[]
    } else if is_uncompressed {
        // SAFETY: `frame` is a live `&[f32]` of `frame.len()` elements, so the byte
        // range `[ptr, ptr + frame.len() * 4)` is entirely inside one allocation and
        // initialised. The length is derived from `frame.len()` rather than from
        // `OPUS_FRAME_SAMPLES`, so the read cannot outrun the slice even if the guard
        // above is ever relaxed. `u8` has alignment 1, so no alignment requirement is
        // introduced, and the borrow of `frame` outlives `payload_bytes`.
        //
        // The result is native-endian, which is the wire format both ends agree on —
        // see `FORMAT_UNCOMPRESSED` in `audio/mod.rs`.
        unsafe {
            std::slice::from_raw_parts(frame.as_ptr() as *const u8, std::mem::size_of_val(frame))
        }
    } else {
        let encoder = encoder.ok_or_else(|| {
            AudioError::CaptureInstanceFailed("compressed stream has no Opus encoder".into())
        })?;
        let encoded_len = encoder.encode_float(frame, opus_output).map_err(|source| {
            AudioError::OpusCodecFailed {
                direction: CodecDirection::Encoder,
                source,
            }
        })?;
        &opus_output[..encoded_len]
    };

    packet_buf.clear();
    packet_buf.extend_from_slice(&seq_num.to_be_bytes());
    packet_buf.push(format_flag);
    packet_buf.extend_from_slice(payload_bytes);

    Ok(EncodeResult::Encoded)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::{
        FORMAT_FLAG_SIZE, FORMAT_OPUS, FORMAT_SILENCE, FORMAT_UNCOMPRESSED, MAX_OPUS_PACKET_SIZE,
        OPUS_FRAME_SAMPLES, SEQ_NUM_SIZE,
    };

    fn make_encoder() -> Encoder {
        crate::audio::create_opus_encoder().unwrap()
    }

    #[test]
    fn encode_frame_should_produce_silence_flag_for_quiet_audio() {
        let frame = vec![0.0f32; OPUS_FRAME_SAMPLES];
        let mut opus_out = vec![0u8; MAX_OPUS_PACKET_SIZE];
        let mut packet = Vec::new();

        let result = encode_frame(
            &frame,
            None,
            AudioBitrate::Opus(128_000),
            5,
            &mut opus_out,
            &mut packet,
        )
        .unwrap();
        assert!(matches!(result, EncodeResult::Encoded));
        assert_eq!(packet[SEQ_NUM_SIZE], FORMAT_SILENCE);
        assert_eq!(packet.len(), SEQ_NUM_SIZE + FORMAT_FLAG_SIZE); // no payload
    }

    #[test]
    fn encode_frame_should_produce_uncompressed_flag_when_no_bitrate() {
        let mut frame = vec![0.0f32; OPUS_FRAME_SAMPLES];
        frame[0] = 0.5; // non-silent
        frame[1] = 0.5;
        let mut opus_out = vec![0u8; MAX_OPUS_PACKET_SIZE];
        let mut packet = Vec::new();

        let result = encode_frame(
            &frame,
            None,
            AudioBitrate::Uncompressed,
            10,
            &mut opus_out,
            &mut packet,
        )
        .unwrap();
        assert!(matches!(result, EncodeResult::Encoded));
        assert_eq!(packet[SEQ_NUM_SIZE], FORMAT_UNCOMPRESSED);
    }

    #[test]
    fn encode_frame_should_produce_opus_flag_for_normal_audio() {
        let mut encoder = make_encoder();
        let frame = vec![0.1f32; OPUS_FRAME_SAMPLES];
        let mut opus_out = vec![0u8; MAX_OPUS_PACKET_SIZE];
        let mut packet = Vec::new();

        let result = encode_frame(
            &frame,
            Some(&mut encoder),
            AudioBitrate::Opus(128_000),
            7,
            &mut opus_out,
            &mut packet,
        )
        .unwrap();
        assert!(matches!(result, EncodeResult::Encoded));
        assert_eq!(packet[SEQ_NUM_SIZE], FORMAT_OPUS);
        assert!(packet.len() > SEQ_NUM_SIZE + FORMAT_FLAG_SIZE); // has opus payload
    }

    #[test]
    fn encode_frame_should_prepend_sequence_number() {
        let frame = vec![0.0f32; OPUS_FRAME_SAMPLES];
        let mut opus_out = vec![0u8; MAX_OPUS_PACKET_SIZE];
        let mut packet = Vec::new();

        encode_frame(
            &frame,
            None,
            AudioBitrate::Uncompressed,
            0xDEAD,
            &mut opus_out,
            &mut packet,
        )
        .unwrap();
        let seq = u64::from_be_bytes(packet[..8].try_into().unwrap());
        assert_eq!(seq, 0xDEAD);
    }

    #[test]
    fn encode_frame_should_include_correct_uncompressed_payload_length() {
        let mut frame = vec![0.0f32; OPUS_FRAME_SAMPLES];
        frame[0] = 0.5; // non-silent
        frame[1] = 0.5;
        let mut opus_out = vec![0u8; MAX_OPUS_PACKET_SIZE];
        let mut packet = Vec::new();

        encode_frame(
            &frame,
            None,
            AudioBitrate::Uncompressed,
            1,
            &mut opus_out,
            &mut packet,
        )
        .unwrap();

        // Uncompressed payload = OPUS_FRAME_SAMPLES * 4 bytes per f32
        let expected_payload = OPUS_FRAME_SAMPLES * std::mem::size_of::<f32>();
        let actual_payload = packet.len() - SEQ_NUM_SIZE - FORMAT_FLAG_SIZE;
        assert_eq!(
            actual_payload, expected_payload,
            "Uncompressed payload should be {} bytes, got {}",
            expected_payload, actual_payload
        );
    }

    mod frame_length_guard {
        use super::*;

        fn encode_len(len: usize, bitrate: AudioBitrate) -> Result<EncodeResult, GemaCastError> {
            let mut frame = vec![0.0f32; len];
            // Non-silent, so the guard is what rejects it rather than the silence
            // branch short-circuiting before the payload is ever built.
            for (i, sample) in frame.iter_mut().enumerate() {
                *sample = if i % 2 == 0 { 0.5 } else { -0.5 };
            }
            let mut opus_out = vec![0u8; MAX_OPUS_PACKET_SIZE];
            let mut packet = Vec::new();
            encode_frame(&frame, None, bitrate, 1, &mut opus_out, &mut packet)
        }

        #[test]
        fn should_reject_a_short_frame_rather_than_transmit_a_misframed_packet() {
            // One sample-pair short. Without the guard the old code read
            // `OPUS_FRAME_SAMPLES * 4` bytes from a 958-element slice — 8 bytes past
            // the end — and the receiver would have accepted the packet as valid.
            let result = encode_len(OPUS_FRAME_SAMPLES - 2, AudioBitrate::Uncompressed);

            match result {
                Err(GemaCastError::Audio(AudioError::InvalidFrameLength { got, expected })) => {
                    assert_eq!(got, OPUS_FRAME_SAMPLES - 2);
                    assert_eq!(expected, OPUS_FRAME_SAMPLES);
                }
                other => panic!("expected InvalidFrameLength, got {other:?}"),
            }
        }

        #[test]
        fn should_reject_an_empty_frame_before_the_rms_divides_by_its_length() {
            // The RMS divisor is `frame.len()`, so an empty frame would yield NaN,
            // `NaN < threshold` is false, and the encoder would be handed nothing.
            assert!(matches!(
                encode_len(0, AudioBitrate::Uncompressed),
                Err(GemaCastError::Audio(AudioError::InvalidFrameLength {
                    got: 0,
                    ..
                }))
            ));
        }

        #[test]
        fn should_reject_a_long_frame_on_the_opus_path_too() {
            // Opus would reject 1920 samples itself, but with a different error and
            // only after the RMS scan. The guard has to be format-independent because
            // 960 is a property of the wire protocol, not of the codec.
            assert!(matches!(
                encode_len(OPUS_FRAME_SAMPLES * 2, AudioBitrate::Opus(128_000)),
                Err(GemaCastError::Audio(AudioError::InvalidFrameLength { .. }))
            ));
        }

        #[test]
        fn should_accept_exactly_one_frame() {
            assert!(encode_len(OPUS_FRAME_SAMPLES, AudioBitrate::Uncompressed).is_ok());
        }
    }

    mod silence_threshold {
        use super::*;

        #[test]
        fn should_measure_rms_over_the_frame_length_it_was_given() {
            // A frame that is half full-scale and half zero has RMS 0.5/sqrt(2), well
            // above the threshold. Dividing by a hard-coded 960 rather than
            // `frame.len()` gives the same answer only while the two agree — this
            // pins the arithmetic, not the constant.
            let mut frame = vec![0.0f32; OPUS_FRAME_SAMPLES];
            for sample in frame.iter_mut().take(OPUS_FRAME_SAMPLES / 2) {
                *sample = 1.0;
            }
            let mut opus_out = vec![0u8; MAX_OPUS_PACKET_SIZE];
            let mut packet = Vec::new();

            encode_frame(
                &frame,
                None,
                AudioBitrate::Uncompressed,
                1,
                &mut opus_out,
                &mut packet,
            )
            .unwrap();

            assert_eq!(
                packet[SEQ_NUM_SIZE], FORMAT_UNCOMPRESSED,
                "loud audio must not be classified as silence"
            );
        }

        #[test]
        fn should_treat_a_signal_below_the_threshold_as_silence() {
            // Just under 0.0001 on every sample, so RMS is just under the threshold.
            let frame = vec![0.00009f32; OPUS_FRAME_SAMPLES];
            let mut opus_out = vec![0u8; MAX_OPUS_PACKET_SIZE];
            let mut packet = Vec::new();

            encode_frame(
                &frame,
                None,
                AudioBitrate::Uncompressed,
                1,
                &mut opus_out,
                &mut packet,
            )
            .unwrap();

            assert_eq!(packet[SEQ_NUM_SIZE], FORMAT_SILENCE);
            assert_eq!(packet.len(), SEQ_NUM_SIZE + FORMAT_FLAG_SIZE);
        }
    }
}
