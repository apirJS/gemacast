use opus::{Decoder, Encoder};

pub const OPUS_CHANNELS: u16 = 2;
pub const OPUS_SAMPLE_RATE: u32 = 48_000;

pub const OPUS_BITRATE: usize = 128_000;
pub const OPUS_FRAME_SIZE: usize = 960;
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

#[cfg(test)]
mod tests {
    use crate::audio::FrameAccumulator;

    #[test]
    fn buffer_drained_correctly() {
        let frame_size: usize = 1024;
        let mut frame_acc = FrameAccumulator::new(frame_size);

        let samples_a = vec![1.0; frame_size + 1];
        let frames_a = frame_acc.push(&samples_a);

        assert_eq!(frames_a.len(), 1);
        assert_eq!(frames_a[0].len(), frame_size);
        assert_eq!(frame_acc.buffer.len(), 1);

        let samples_b = vec![1.0; 1];
        let frames_b = frame_acc.push(&samples_b);

        assert_eq!(frames_b.len(), 0);
        assert_eq!(frame_acc.buffer.len(), 2);

        let samples_c = vec![1.0; frame_size - 2];
        let frames_c = frame_acc.push(&samples_c);

        assert_eq!(frames_c.len(), 1);
        assert_eq!(frames_c[0].len(), frame_size);
        assert_eq!(frame_acc.buffer.len(), 0);
    }
}
