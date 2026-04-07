use std::collections::VecDeque;

use opus::Decoder;
use ringbuf::{HeapCons, traits::*};

use crate::audio::{OPUS_CHANNELS, OPUS_FRAME_SAMPLES};

use super::buffer::JitterBuffer;
use super::types::RawPacket;

const TARGET_DEPTH: u32 = 6;
const MAX_DEPTH: u32 = 20;
const MAX_MISSING: u32 = 5;

/// Coordinates the full jitter buffer pipeline.
///
/// Owns all components (buffer, Opus decoder) and exposes
/// a simple two-method API:
///
/// 1. `ingest_packets()` — drain raw packets from the SPSC channel into the ordered buffer.
/// 2. `fill_output()` — produce exactly the requested number of PCM samples for the cpal callback.
///
/// # Thread model
///
/// This entire struct lives inside the cpal output callback closure.
/// It is **never** shared across threads. The network thread communicates
/// via the lock-free `HeapCons<RawPacket>` ring buffer.
pub struct JitterBufferManager {
    decoder: Decoder,
    buffer: JitterBuffer,
    /// Accumulator of processed PCM samples ready for cpal to consume.
    /// Decouples the Opus frame size (960 samples) from cpal's variable buffer size.
    playback_buf: VecDeque<f32>,
    /// Reusable buffer for Opus decode output (avoids per-frame allocation).
    /// IMPORTANT: Always kept at full capacity (OPUS_FRAME_SAMPLES) — never truncated.
    decode_buf: Vec<f32>,
    /// How many valid samples are in decode_buf after the last decode.
    decode_len: usize,

    is_prebuffering: bool,
    missing_count: u32,
}

impl JitterBufferManager {
    pub fn new(decoder: Decoder) -> Self {
        Self {
            decoder,
            buffer: JitterBuffer::new(),
            playback_buf: VecDeque::with_capacity(OPUS_FRAME_SAMPLES * 4),
            decode_buf: vec![0.0f32; OPUS_FRAME_SAMPLES],
            decode_len: 0,
            is_prebuffering: true,
            missing_count: 0,
        }
    }

    /// Drain all pending raw packets from the SPSC channel into the jitter buffer.
    ///
    /// Called at the top of every cpal callback. The consumer is the audio-thread
    /// side of the lock-free ring buffer shared with the network thread.
    pub fn ingest_packets(&mut self, consumer: &mut HeapCons<RawPacket>) {
        while let Some(pkt) = consumer.try_pop() {
            self.buffer.insert(pkt);
        }
    }

    /// Fill `output` with PCM samples, applying jitter compensation.
    ///
    /// This is the cpal callback's main path. It will:
    /// 1. Pull from the internal playback buffer if data is available.
    /// 2. When the playback buffer is empty, process the next jitter buffer frame
    ///    (decode, time-stretch, or conceal) to refill it.
    /// 3. Scale each sample by `volume` (0.0–1.0).
    ///
    /// If we're in prebuffering mode (not enough depth yet), outputs silence.
    pub fn fill_output(&mut self, output: &mut [f32], volume: f32) {
        for sample in output.iter_mut() {
            if self.playback_buf.is_empty() {
                self.process_next_frame();
            }
            *sample = self.playback_buf.pop_front().unwrap_or(0.0) * volume;
        }
    }

    /// Process one Opus frame from the jitter buffer into the playback buffer.
    fn process_next_frame(&mut self) {
        if self.is_prebuffering {
            if self.buffer.contiguous_depth() >= TARGET_DEPTH {
                self.is_prebuffering = false;
            } else {
                self.playback_buf
                    .extend(std::iter::repeat_n(0.0, OPUS_FRAME_SAMPLES));
                return;
            }
        }

        let depth = self.buffer.contiguous_depth();

        // Bufferbloat protection: Network surged and dumped massive stale queue.
        if depth > MAX_DEPTH {
            self.trigger_reset();
            self.playback_buf
                .extend(std::iter::repeat_n(0.0, OPUS_FRAME_SAMPLES));
            return;
        }

        if let Some(pkt) = self.buffer.pop_next() {
            self.missing_count = 0;
            self.decode_and_buffer(&pkt.opus_data);
        } else {
            self.missing_count += 1;
            if self.missing_count > MAX_MISSING {
                // Severe packet loss: network disconnect or massive interference.
                self.trigger_reset();
                self.playback_buf
                    .extend(std::iter::repeat_n(0.0, OPUS_FRAME_SAMPLES));
            } else {
                self.generate_plc();
            }
        }
    }

    fn trigger_reset(&mut self) {
        self.buffer.reset();
        self.is_prebuffering = true;
        self.missing_count = 0;
        self.playback_buf.clear();
        self.decode_buf.fill(0.0);
        self.decode_len = 0;
        let _ = self.decoder.reset_state();
    }

    /// Decode an Opus packet and append the PCM to the playback buffer.
    fn decode_and_buffer(&mut self, opus_data: &[u8]) {
        if self.decode_opus(opus_data) {
            self.playback_buf
                .extend(&self.decode_buf[..self.decode_len]);
        } else {
            self.generate_plc();
        }
    }

    /// Decode an Opus packet into `self.decode_buf`.
    /// Returns `true` on success.
    ///
    /// IMPORTANT: We track the valid length separately in `self.decode_len`
    /// instead of truncating the Vec. Truncating would shrink the slice
    /// passed to subsequent `decode_float` calls, potentially causing failures
    /// if a prior decode returned fewer samples than expected.
    fn decode_opus(&mut self, opus_data: &[u8]) -> bool {
        match self
            .decoder
            .decode_float(opus_data, &mut self.decode_buf, false)
        {
            Ok(samples_per_channel) => {
                self.decode_len = samples_per_channel * OPUS_CHANNELS as usize;
                true
            }
            Err(_) => false,
        }
    }

    /// Use Opus's native PLC to hallucinate a missing frame.
    ///
    /// This leverages the decoder's internal state from the last good packet
    /// to synthesize plausible audio. Much better than silence — it preserves
    /// the spectral envelope and pitch.
    fn generate_plc(&mut self) {
        match self
            .decoder
            .decode_float(&[] as &[u8], &mut self.decode_buf, false)
        {
            Ok(samples_per_channel) => {
                let total = samples_per_channel * OPUS_CHANNELS as usize;
                self.playback_buf.extend(&self.decode_buf[..total]);
            }
            Err(_) => {
                // Last resort: silence.
                self.playback_buf
                    .extend(std::iter::repeat_n(0.0, OPUS_FRAME_SAMPLES));
            }
        }
    }

    /// Reset all state. Called on disconnect/reconnect.
    pub fn reset(&mut self) {
        self.trigger_reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;
    use opus::{Encoder, Decoder, Channels, Application};
    use ringbuf::HeapRb;
    use crate::audio::{OPUS_SAMPLE_RATE, OPUS_FRAME_SAMPLES};

    fn setup_env() -> (JitterBufferManager, Encoder, ringbuf::HeapProd<RawPacket>, ringbuf::HeapCons<RawPacket>) {
        let decoder = Decoder::new(OPUS_SAMPLE_RATE, Channels::Stereo).unwrap();
        let encoder = Encoder::new(OPUS_SAMPLE_RATE, Channels::Stereo, Application::Audio).unwrap();
        let manager = JitterBufferManager::new(decoder);
        let rb = HeapRb::<RawPacket>::new(100);
        let (prod, cons) = rb.split();
        (manager, encoder, prod, cons)
    }

    fn make_packet(encoder: &mut Encoder, seq: u64) -> RawPacket {
        let pcm = vec![0.0f32; OPUS_FRAME_SAMPLES];
        let opus_data = encoder.encode_vec_float(&pcm, 1500).unwrap();
        RawPacket {
            seq_num: seq,
            opus_data,
            arrival_time: Instant::now(),
        }
    }

    #[test]
    fn test_prebuffering_outputs_silence_until_target_depth() {
        let (mut manager, mut encoder, mut prod, mut cons) = setup_env();

        for i in 1..=5 {
            assert!(prod.try_push(make_packet(&mut encoder, i)).is_ok());

        }
        manager.ingest_packets(&mut cons);

        let mut output = vec![1.0; OPUS_FRAME_SAMPLES];
        manager.fill_output(&mut output, 1.0);

        for &sample in &output {
            assert_eq!(sample, 0.0);
        }
        assert!(manager.is_prebuffering);

        assert!(prod.try_push(make_packet(&mut encoder, 6)).is_ok());
        manager.ingest_packets(&mut cons);

        manager.fill_output(&mut output, 1.0);
        assert!(!manager.is_prebuffering);
    }

    #[test]
    fn test_packet_loss_triggers_plc() {
        let (mut manager, mut encoder, mut prod, mut cons) = setup_env();

        for i in 1..=6 {
            assert!(prod.try_push(make_packet(&mut encoder, i)).is_ok());

        }
        manager.ingest_packets(&mut cons);

        let mut output = vec![0.0; OPUS_FRAME_SAMPLES];
        manager.fill_output(&mut output, 1.0);
        assert!(!manager.is_prebuffering);

        assert!(prod.try_push(make_packet(&mut encoder, 8)).is_ok());
        manager.ingest_packets(&mut cons);

        for _ in 2..=6 {
            manager.fill_output(&mut output, 1.0);
        }

        manager.fill_output(&mut output, 1.0);
        assert_eq!(manager.missing_count, 1);
        assert!(!manager.is_prebuffering);
    }

    #[test]
    fn test_excessive_packet_loss_triggers_reset() {
        let (mut manager, mut encoder, mut prod, mut cons) = setup_env();

        for i in 1..=6 {
            assert!(prod.try_push(make_packet(&mut encoder, i)).is_ok());

        }
        manager.ingest_packets(&mut cons);

        let mut output = vec![0.0; OPUS_FRAME_SAMPLES];
        for _ in 1..=6 {
            manager.fill_output(&mut output, 1.0);
        }
        assert!(!manager.is_prebuffering);

        for i in 1..=5 {
            manager.fill_output(&mut output, 1.0);
            assert_eq!(manager.missing_count, i);
            assert!(!manager.is_prebuffering);
        }

        manager.fill_output(&mut output, 1.0);
        assert!(manager.is_prebuffering);
    }

    #[test]
    fn test_bufferbloat_protection() {
        let (mut manager, mut encoder, mut prod, mut cons) = setup_env();

        for i in 1..=25 {
            assert!(prod.try_push(make_packet(&mut encoder, i)).is_ok());

        }
        manager.ingest_packets(&mut cons);

        let mut output = vec![0.0; OPUS_FRAME_SAMPLES];
        manager.fill_output(&mut output, 1.0);

        assert!(manager.is_prebuffering);
        assert_eq!(manager.buffer.contiguous_depth(), 0);
    }
}
