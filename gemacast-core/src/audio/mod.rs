use opus::{Decoder, Encoder};

pub mod resampler;
pub use resampler::CaptureResampler;
pub const OPUS_CHANNELS: u16 = 2;
pub const OPUS_SAMPLE_RATE: u32 = 48_000;

pub const OPUS_BITRATE: usize = 128_000;
pub const OPUS_FRAME_SIZE: usize = 240;
pub const OPUS_FRAME_SAMPLES: usize = OPUS_FRAME_SIZE * OPUS_CHANNELS as usize;

/// Safe bounds for largest possible packet.
/// A full uncompressed PCM frame is 1920 f32s = 7680 bytes.
pub const MAX_OPUS_PACKET_SIZE: usize = 8000;
pub const SEQ_NUM_SIZE: usize = 8;
pub const FORMAT_FLAG_SIZE: usize = 1;

pub const FORMAT_OPUS: u8 = 0;
pub const FORMAT_UNCOMPRESSED: u8 = 1;
pub const FORMAT_SILENCE: u8 = 2;

pub fn create_opus_encoder_with_bitrate(bitrate: i32) -> Result<Encoder, opus::Error> {
    let mut encoder = Encoder::new(
        OPUS_SAMPLE_RATE,
        opus::Channels::Stereo,
        opus::Application::LowDelay,
    )?;

    encoder.set_bitrate(opus::Bitrate::Bits(bitrate))?;
    // Complexity 5 (vs default 10): no perceptible quality loss at >=128kbps,
    encoder.set_complexity(5)?;
    // CELT mode: treated as music/system audio, avoids speech/music detection overhead.
    encoder.set_signal(opus::Signal::Music)?;

    Ok(encoder)
}

pub fn create_opus_encoder() -> Result<Encoder, opus::Error> {
    create_opus_encoder_with_bitrate(OPUS_BITRATE as i32)
}

pub fn create_opus_decoder() -> Result<Decoder, opus::Error> {
    let decoder = Decoder::new(OPUS_SAMPLE_RATE, opus::Channels::Stereo)?;

    Ok(decoder)
}

#[cfg(test)]
mod tests {
    use super::*;

    mod constants {
        use super::*;

        #[test]
        fn opus_frame_samples_should_equal_frame_size_times_channels() {
            assert_eq!(OPUS_FRAME_SAMPLES, OPUS_FRAME_SIZE * OPUS_CHANNELS as usize);
        }

        #[test]
        fn max_packet_size_should_accommodate_uncompressed_pcm_frame() {
            // Uncompressed PCM frame = OPUS_FRAME_SAMPLES * 4 bytes/f32 = 1920 bytes
            let uncompressed_size = OPUS_FRAME_SAMPLES * std::mem::size_of::<f32>();
            assert!(
                MAX_OPUS_PACKET_SIZE >= uncompressed_size,
                "MAX_OPUS_PACKET_SIZE ({}) must fit uncompressed frame ({})",
                MAX_OPUS_PACKET_SIZE,
                uncompressed_size
            );
        }
    }

    mod codec_factories {
        use super::*;

        #[test]
        fn create_opus_encoder_should_succeed() {
            let encoder = create_opus_encoder();
            assert!(
                encoder.is_ok(),
                "Encoder creation failed: {:?}",
                encoder.unwrap_err()
            );
        }

        #[test]
        fn create_opus_decoder_should_succeed() {
            let decoder = create_opus_decoder();
            assert!(
                decoder.is_ok(),
                "Decoder creation failed: {:?}",
                decoder.unwrap_err()
            );
        }

        #[test]
        fn create_opus_encoder_with_custom_bitrate_should_succeed() {
            let encoder = create_opus_encoder_with_bitrate(64_000);
            assert!(
                encoder.is_ok(),
                "Custom bitrate encoder failed: {:?}",
                encoder.unwrap_err()
            );
        }

        #[test]
        fn encode_then_decode_should_produce_correct_sample_count() {
            let mut encoder = create_opus_encoder().unwrap();
            let mut decoder = create_opus_decoder().unwrap();

            let input = vec![0.1f32; OPUS_FRAME_SAMPLES];
            let mut opus_buf = vec![0u8; MAX_OPUS_PACKET_SIZE];
            let encoded_len = encoder.encode_float(&input, &mut opus_buf).unwrap();

            let mut output = vec![0.0f32; OPUS_FRAME_SAMPLES];
            let decoded_samples = decoder
                .decode_float(&opus_buf[..encoded_len], &mut output, false)
                .unwrap();

            assert_eq!(
                decoded_samples, OPUS_FRAME_SIZE,
                "Expected {} decoded frames, got {}",
                OPUS_FRAME_SIZE, decoded_samples
            );
        }
    }
}
