use opus::{Decoder, Encoder};

pub const OPUS_CHANNELS: u16 = 2;
pub const OPUS_SAMPLE_RATE: u32 = 48_000;

pub const OPUS_BITRATE: usize = 128_000;
/// 480 samples @ 48 kHz = 10ms per frame.
/// AudioRelay and other low-latency streamers use 10ms frames to halve
/// the effective Wi-Fi burst aggregation latency vs. the 20ms default.
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

pub fn create_opus_encoder() -> Result<Encoder, opus::Error> {
    let mut encoder = Encoder::new(
        OPUS_SAMPLE_RATE,
        opus::Channels::Stereo,
        opus::Application::LowDelay,
    )?;

    encoder.set_bitrate(opus::Bitrate::Bits(OPUS_BITRATE as i32))?;
    // Complexity 5 (vs default 10): no perceptible quality loss at >=128kbps,
    encoder.set_complexity(5)?;
    // CELT mode: treated as music/system audio, avoids speech/music detection overhead.
    encoder.set_signal(opus::Signal::Music)?;

    Ok(encoder)
}

pub fn create_opus_decoder() -> Result<Decoder, opus::Error> {
    let decoder = Decoder::new(OPUS_SAMPLE_RATE, opus::Channels::Stereo)?;

    Ok(decoder)
}
