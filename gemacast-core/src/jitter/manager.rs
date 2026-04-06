use std::collections::VecDeque;

use opus::Decoder;
use ringbuf::{HeapCons, traits::*};

use crate::audio::{OPUS_CHANNELS, OPUS_FRAME_SAMPLES};

use super::buffer::JitterBuffer;
use super::controller::JitterController;
use super::types::{DspCommand, RawPacket};

/// Coordinates the full jitter buffer pipeline.
///
/// Owns all components (buffer, controller, WSOLA, Opus decoder) and exposes
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
    controller: JitterController,
    /// Accumulator of processed PCM samples ready for cpal to consume.
    /// Decouples the Opus frame size (960 samples) from cpal's variable buffer size.
    playback_buf: VecDeque<f32>,
    /// Reusable buffer for Opus decode output (avoids per-frame allocation).
    /// IMPORTANT: Always kept at full capacity (OPUS_FRAME_SAMPLES) — never truncated.
    decode_buf: Vec<f32>,
    /// How many valid samples are in decode_buf after the last decode.
    decode_len: usize,
}

impl JitterBufferManager {
    pub fn new(decoder: Decoder) -> Self {
        Self {
            decoder,
            buffer: JitterBuffer::new(),
            controller: JitterController::new(),
            playback_buf: VecDeque::with_capacity(OPUS_FRAME_SAMPLES * 4),
            decode_buf: vec![0.0f32; OPUS_FRAME_SAMPLES],
            decode_len: 0,
        }
    }

    /// Drain all pending raw packets from the SPSC channel into the jitter buffer.
    ///
    /// Called at the top of every cpal callback. The consumer is the audio-thread
    /// side of the lock-free ring buffer shared with the network thread.
    pub fn ingest_packets(&mut self, consumer: &mut HeapCons<RawPacket>) {
        while let Some(pkt) = consumer.try_pop() {
            self.controller
                .record_arrival(pkt.seq_num, pkt.arrival_time);
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
        let depth = self.buffer.contiguous_depth();

        // Prebuffering: accumulate frames before starting playback.
        self.controller.check_prebuffer(depth);
        if self.controller.is_prebuffering() {
            // Output one frame of silence while we wait for depth to build.
            self.playback_buf
                .extend(std::iter::repeat_n(0.0, OPUS_FRAME_SAMPLES));
            return;
        }

        let has_next = self.buffer.has_next();
        let command = self.controller.decide(depth, has_next);

        match command {
            DspCommand::Normal => {
                if let Some(pkt) = self.buffer.pop_next() {
                    self.decode_and_buffer(&pkt.opus_data);
                } else {
                    self.generate_plc();
                }
            }
            DspCommand::Accelerate { factor: _ } => {
                // Drop one packet to reduce buffer depth quickly.
                let _ = self.buffer.pop_next();
                
                // Process the *next* packet (or conceal if buffer just became empty)
                if let Some(pkt) = self.buffer.pop_next() {
                    self.decode_and_buffer(&pkt.opus_data);
                } else {
                    self.generate_plc();
                }
            }
            DspCommand::Expand { factor: _ } => {
                // Generate a highly convincing Opus PLC frame to artificially expand buffer depth.
                // We don't advance the sequence cursor here, so the actual next packet 
                // will safely play in the subsequent callback.
                self.generate_plc();
            }
            DspCommand::Conceal => {
                // Advance past the missing frame.
                let _ = self.buffer.pop_next();
                self.generate_plc();
            }
        }
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
        self.buffer.reset();
        self.controller.reset();
        self.playback_buf.clear();
        self.decode_buf.fill(0.0);
        self.decode_len = 0;
    }
}
