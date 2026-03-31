use opus::{Decoder, Encoder};

pub const OPUS_CHANNELS: u16 = 2;
pub const OPUS_SAMPLE_RATE: u32 = 48_000;

pub const OPUS_BITRATE: usize = 128_000;
pub const OPUS_FRAME_SIZE: usize = 480;
pub const OPUS_FRAME_SAMPLES: usize = OPUS_FRAME_SIZE * OPUS_CHANNELS as usize;

/// Maximum possible Opus packet size (spec says 1275 * 3 + 7, but 4000 is safe)
pub const MAX_OPUS_PACKET_SIZE: usize = 4000;
pub const SEQ_NUM_SIZE: usize = 8;

pub fn create_opus_encoder() -> Result<Encoder, opus::Error> {
    let mut encoder = Encoder::new(
        OPUS_SAMPLE_RATE,
        opus::Channels::Stereo,
        opus::Application::LowDelay,
    )?;

    encoder.set_bitrate(opus::Bitrate::Bits(OPUS_BITRATE as i32))?;

    Ok(encoder)
}

pub fn create_opus_decoder() -> Result<Decoder, opus::Error> {
    let decoder = Decoder::new(OPUS_SAMPLE_RATE, opus::Channels::Stereo)?;

    Ok(decoder)
}

pub struct FrameAccumulator {
    pub buffer: Vec<f32>,
    pub frame_size: usize,
}

impl FrameAccumulator {
    pub fn new(frame_size: usize) -> Self {
        Self {
            buffer: Vec::with_capacity(frame_size * 2),
            frame_size,
        }
    }

    pub fn push(&mut self, samples: &[f32]) -> Vec<Vec<f32>> {
        self.buffer.extend_from_slice(samples);

        let mut frames = Vec::new();
        while self.buffer.len() >= self.frame_size {
            let frame: Vec<f32> = self.buffer.drain(..self.frame_size).collect();
            frames.push(frame);
        }

        frames
    }
}
