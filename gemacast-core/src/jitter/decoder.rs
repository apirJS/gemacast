//! Decode actor: owns the Opus decoder and the reusable PCM decode buffer.
//! Turns a `RawPacket` (Opus, raw PCM, or silence) into decoded f32 samples,
//! handling packet-loss concealment (PLC) and gap-aware decoder-state warming.
//! Owns no jitter buffer or playback buffer — it only decodes into `decode_buf`.

use super::types::RawPacket;
use crate::audio::{OPUS_CHANNELS, OPUS_FRAME_SAMPLES};
use opus::Decoder;

/// Opus decoder + reusable decode buffer for one audio callback thread.
pub(super) struct FrameDecoder {
    decoder: Decoder,
    /// Reusable buffer for Opus decode output (avoids per-frame allocation).
    /// IMPORTANT: Always kept at full capacity (OPUS_FRAME_SAMPLES) — never truncated.
    pub decode_buf: Vec<f32>,
    /// How many valid samples are in decode_buf after the last decode.
    pub decode_len: usize,
    /// Tracks the exact sequence number the Opus predictive state machine is calibrated for.
    pub opus_next_expected_seq: Option<u64>,
}

impl FrameDecoder {
    pub fn new(decoder: Decoder) -> Self {
        Self {
            decoder,
            decode_buf: vec![0.0f32; OPUS_FRAME_SAMPLES],
            decode_len: 0,
            opus_next_expected_seq: None,
        }
    }

    /// Decode a packet's payload into `self.decode_buf[..self.decode_len]`.
    ///
    /// Zero-allocation: all output goes into the pre-allocated decode buffer.
    /// Silence frames output zeros without touching the decoder state.
    /// Uncompressed PCM frames are copied directly without decoder interaction.
    pub fn capture(&mut self, pkt: &RawPacket) {
        if let Some(expected) = self.opus_next_expected_seq
            && pkt.seq_num != expected
        {
            let gap = pkt.seq_num.saturating_sub(expected);
            if gap > 20 {
                // Large discontinuity (>100ms): full decoder reset.
                let _ = self.decoder.reset_state();
            } else if gap > 0 && gap <= 5 {
                // Small forward gap (5-25ms): feed PLC frames to keep decoder
                // state warm for smooth concealment. This prevents the hard
                // transient click that reset_state() would cause.
                for _ in 0..gap {
                    let _ = self.decoder.decode_float(&[], &mut self.decode_buf, false);
                }
            }
            // Gaps 6-20: decoder continues without intervention.
            // PLC quality degrades naturally but no hard reset click.
        }

        if pkt.is_silence {
            // Silence is intentional (sender detected quiet audio), not a loss
            // event. Don't feed PLC — it would poison the decoder's internal
            // state with hallucinated spectral data, causing a brief "warble"
            // artifact when real audio resumes.
            self.decode_buf[..OPUS_FRAME_SAMPLES].fill(0.0);
            self.decode_len = OPUS_FRAME_SAMPLES;
        } else if pkt.is_uncompressed {
            let f32_len = pkt.payload_len / std::mem::size_of::<f32>();
            if f32_len == 0 {
                // Empty uncompressed payload — generate PLC as fallback
                self.decode_plc();
            } else {
                // Copy raw PCM directly without decoder interaction.
                // Don't feed PLC — uncompressed frames are a format choice,
                // not a loss event. Mixing PLC state into a non-Opus path
                // only poisons future Opus decode transitions.
                for (i, chunk) in pkt.payload_data[..pkt.payload_len]
                    .chunks_exact(4)
                    .enumerate()
                {
                    self.decode_buf[i] = f32::from_ne_bytes(chunk.try_into().unwrap());
                }
                self.decode_len = f32_len.min(self.decode_buf.len());
            }
        } else if !self.decode_opus(&pkt.payload_data[..pkt.payload_len]) {
            self.decode_plc();
        }
        self.opus_next_expected_seq = Some(pkt.seq_num + 1);
    }

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

    /// Generate one PLC (packet-loss-concealment) frame into `decode_buf`.
    pub fn decode_plc(&mut self) {
        match self
            .decoder
            .decode_float(&[] as &[u8], &mut self.decode_buf, false)
        {
            Ok(samples_per_channel) => {
                self.decode_len = samples_per_channel * OPUS_CHANNELS as usize;
            }
            Err(_) => {
                self.decode_buf.fill(0.0);
                self.decode_len = OPUS_FRAME_SAMPLES;
            }
        }
    }

    /// Reset the decoder's predictive state and forget the expected sequence.
    /// Used on stream restart and large fast-forward jumps.
    pub fn resync(&mut self) {
        let _ = self.decoder.reset_state();
        self.opus_next_expected_seq = None;
    }

    /// Full reset on stream teardown: zero the decode buffer and reset decoder
    /// state. Matches the legacy `trigger_reset` decoder handling, which
    /// deliberately leaves `opus_next_expected_seq` untouched.
    pub fn reset(&mut self) {
        self.decode_buf.fill(0.0);
        self.decode_len = 0;
        let _ = self.decoder.reset_state();
    }
}
