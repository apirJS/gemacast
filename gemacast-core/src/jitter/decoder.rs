//! Decode actor: owns the Opus decoder and the reusable PCM decode buffer.
//! Turns a `RawPacket` (Opus, raw PCM, or silence) into decoded f32 samples,
//! handling packet-loss concealment (PLC) and gap-aware decoder-state warming.
//! Owns no jitter buffer or playback buffer — it only decodes into `decode_buf`.

use super::types::RawPacket;
use crate::audio::{OPUS_CHANNELS, OPUS_FRAME_SAMPLES};
use opus::Decoder;

/// Wire format of the most recently captured packet.
///
/// Reported on the 1 Hz depth line as `fmt=`. Recorded explicitly because
/// nothing else in the log states it: inferring the format from the separate
/// `Latency: … RMS:` heartbeat only works because a packet-size proxy pins at
/// 1.0 on the Opus path while a real PCM RMS does not, and that is a
/// coincidence of two distributions, not a property to rely on.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(super) enum PacketFormat {
    /// No packet has been captured yet (startup, or a whole window of outage).
    #[default]
    None,
    Opus,
    Uncompressed,
    Silence,
}

impl PacketFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Opus => "opus",
            Self::Uncompressed => "unc",
            Self::Silence => "sil",
        }
    }
}

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
    /// Wire format of the last packet `capture` saw. Observation only — nothing
    /// branches on it; it exists so a field capture states its own format.
    pub last_format: PacketFormat,
    /// Whether the Opus decoder's internal state corresponds to the frame we
    /// most recently played, and therefore whether [`Self::decode_plc`] can
    /// extrapolate from it.
    ///
    /// This is an exact fact about what the decoder has been fed, not a
    /// heuristic. On the uncompressed and silence paths `capture` never calls
    /// into the codec, so a PLC request there runs on a state that was either
    /// never initialised — which returns exact zeros, measured as `rms=0` for
    /// every concealed frame of an uncompressed capture — or is stale by however
    /// many frames have played since the last Opus packet.
    ///
    /// It has to mean "does the state match the last frame", not "has the
    /// decoder ever seen Opus": a mid-session `/change-bitrate` from Opus to
    /// uncompressed leaves a decoder that has been fed, but whose state drifts
    /// further from what is playing with every frame. Extrapolating from that is
    /// worse than repeating audio that actually played.
    pub plc_is_valid: bool,
}

impl FrameDecoder {
    pub fn new(decoder: Decoder) -> Self {
        Self {
            decoder,
            decode_buf: vec![0.0f32; OPUS_FRAME_SAMPLES],
            decode_len: 0,
            opus_next_expected_seq: None,
            last_format: PacketFormat::None,
            plc_is_valid: false,
        }
    }

    /// The valid decoded samples from the last `capture`/`decode_plc`.
    pub fn decoded(&self) -> &[f32] {
        &self.decode_buf[..self.decode_len]
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

        self.last_format = if pkt.is_silence {
            PacketFormat::Silence
        } else if pkt.is_uncompressed {
            PacketFormat::Uncompressed
        } else {
            PacketFormat::Opus
        };

        if pkt.is_silence {
            // Silence is intentional (sender detected quiet audio), not a loss
            // event. Don't feed PLC — it would poison the decoder's internal
            // state with hallucinated spectral data, causing a brief "warble"
            // artifact when real audio resumes.
            self.decode_buf[..OPUS_FRAME_SAMPLES].fill(0.0);
            self.decode_len = OPUS_FRAME_SAMPLES;
            // The codec was not advanced, so its state no longer describes what
            // is playing. Concealment must repeat played audio, not extrapolate.
            self.plc_is_valid = false;
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
            // Set after the branch so it also covers the empty-payload fallback
            // above: that `decode_plc` call runs on a codec this stream has never
            // fed, so the frame it "predicts" is digital silence, and claiming a
            // valid state from it would hide exactly this case.
            self.plc_is_valid = false;
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
                self.plc_is_valid = true;
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
                // A PLC call is itself a state advance: the codec now describes
                // the frame it just predicted, which is the frame being played.
                self.plc_is_valid = true;
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
        // `reset_state` discards the prediction history, so a PLC frame taken
        // before the next successful decode would be extrapolated from nothing.
        // Same fact as the uncompressed branch, reached by a different door.
        self.plc_is_valid = false;
    }

    /// Full reset on stream teardown: zero the decode buffer and reset decoder
    /// state. Matches the legacy `trigger_reset` decoder handling, which
    /// deliberately leaves `opus_next_expected_seq` untouched.
    pub fn reset(&mut self) {
        self.decode_buf.fill(0.0);
        self.decode_len = 0;
        self.last_format = PacketFormat::None;
        self.plc_is_valid = false;
        let _ = self.decoder.reset_state();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::OPUS_SAMPLE_RATE;
    use opus::{Application, Channels, Encoder};

    fn decoder() -> FrameDecoder {
        FrameDecoder::new(Decoder::new(OPUS_SAMPLE_RATE, Channels::Stereo).unwrap())
    }

    fn tone_pcm(seq: u64) -> Vec<f32> {
        let ch = OPUS_CHANNELS as usize;
        let frames = OPUS_FRAME_SAMPLES / ch;
        let mut pcm = vec![0.0f32; OPUS_FRAME_SAMPLES];
        let base = seq * frames as u64;
        for i in 0..frames {
            let t = (base + i as u64) as f32 / OPUS_SAMPLE_RATE as f32;
            let s = (2.0 * std::f32::consts::PI * 200.0 * t).sin() * 0.5;
            for c in 0..ch {
                pcm[i * ch + c] = s;
            }
        }
        pcm
    }

    fn opus_packet(encoder: &mut Encoder, seq: u64) -> RawPacket {
        let d = encoder.encode_vec_float(&tone_pcm(seq), 1500).unwrap();
        let mut pkt = RawPacket::zeroed();
        pkt.seq_num = seq;
        pkt.payload_len = d.len();
        pkt.payload_data[..d.len()].copy_from_slice(&d);
        pkt
    }

    fn uncompressed_packet(seq: u64) -> RawPacket {
        let pcm = tone_pcm(seq);
        let mut pkt = RawPacket::zeroed();
        pkt.seq_num = seq;
        pkt.is_uncompressed = true;
        pkt.payload_len = pcm.len() * std::mem::size_of::<f32>();
        for (i, s) in pcm.iter().enumerate() {
            pkt.payload_data[i * 4..i * 4 + 4].copy_from_slice(&s.to_ne_bytes());
        }
        pkt
    }

    fn silence_packet(seq: u64) -> RawPacket {
        let mut pkt = RawPacket::zeroed();
        pkt.seq_num = seq;
        pkt.is_silence = true;
        pkt
    }

    /// A fresh decoder has been fed nothing, so it cannot extrapolate anything —
    /// and this is the exact state every uncompressed stream's concealment used
    /// to run on, which is why concealed frames measured as digital silence.
    /// Pinned separately from the transitions below because `Default`-ing this
    /// flag to `true` would reintroduce the whole defect while leaving both
    /// transition tests green.
    #[test]
    fn a_decoder_that_has_never_been_fed_should_not_claim_a_usable_plc_state() {
        let dec = decoder();
        assert!(!dec.plc_is_valid);

        // And the reason it matters, measured rather than asserted from the doc:
        // PLC on that state returns exact zeros.
        let mut dec = dec;
        dec.decode_plc();
        let peak = dec.decoded().iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert_eq!(
            peak, 0.0,
            "the premise of the pitch-repetition path is that this frame is \
             digital silence; if libopus ever starts extrapolating from a virgin \
             state, the gate is solving a problem that no longer exists",
        );
    }

    /// The gate has to be "does the codec state describe the frame we just
    /// played", not "has this decoder ever seen Opus". A `/change-bitrate` switch
    /// mid-session leaves a decoder that *has* been fed and whose state drifts one
    /// frame further from the truth on every uncompressed frame after it — so the
    /// interesting case is the one where the flag has to go back down.
    #[test]
    fn an_uncompressed_packet_should_invalidate_the_opus_decoders_plc_state() {
        let mut enc = Encoder::new(OPUS_SAMPLE_RATE, Channels::Stereo, Application::Audio).unwrap();
        let mut dec = decoder();

        dec.capture(&opus_packet(&mut enc, 1));
        assert!(
            dec.plc_is_valid,
            "precondition: a decoded Opus frame must leave the codec able to \
             extrapolate, or the test below cannot observe a transition",
        );
        assert_eq!(dec.last_format, PacketFormat::Opus);

        dec.capture(&uncompressed_packet(2));
        assert!(
            !dec.plc_is_valid,
            "the codec was not advanced by the PCM copy, so its state now \
             describes frame 1 while frame 2 is what played",
        );
        assert_eq!(dec.last_format, PacketFormat::Uncompressed);

        // ...and back, so the flag tracks the format rather than latching either way.
        dec.capture(&opus_packet(&mut enc, 3));
        assert!(dec.plc_is_valid);
    }

    /// Same fact on the silence path, which is a separate branch: a silence frame
    /// deliberately bypasses the codec to avoid poisoning it, and that bypass is
    /// exactly what makes the state stale.
    #[test]
    fn a_silence_frame_should_invalidate_the_opus_decoders_plc_state() {
        let mut enc = Encoder::new(OPUS_SAMPLE_RATE, Channels::Stereo, Application::Audio).unwrap();
        let mut dec = decoder();

        dec.capture(&opus_packet(&mut enc, 1));
        assert!(dec.plc_is_valid, "precondition");

        dec.capture(&silence_packet(2));
        assert!(!dec.plc_is_valid);
        assert_eq!(dec.last_format, PacketFormat::Silence);
    }

    /// `reset_state` discards the prediction history, so a PLC frame taken before
    /// the next successful decode is extrapolated from nothing — the same defect
    /// as the uncompressed path, reached through the resync door instead. The flag
    /// is defined by what the codec knows, and `resync` is a place where it stops
    /// knowing it.
    #[test]
    fn a_resync_should_invalidate_the_opus_decoders_plc_state() {
        let mut enc = Encoder::new(OPUS_SAMPLE_RATE, Channels::Stereo, Application::Audio).unwrap();
        let mut dec = decoder();

        dec.capture(&opus_packet(&mut enc, 1));
        assert!(dec.plc_is_valid, "precondition");

        dec.resync();
        assert!(!dec.plc_is_valid);

        dec.capture(&opus_packet(&mut enc, 7));
        assert!(
            dec.plc_is_valid,
            "and it must recover on the next real decode — a gate that could not \
             re-arm would strand the Opus path on pitch repetition forever",
        );
    }

    /// The empty-uncompressed sub-branch calls `decode_plc`, which sets the flag
    /// on its way out. The assignment therefore has to sit *after* the branch, or
    /// a zero-length PCM payload would claim a valid state built from digital
    /// silence.
    #[test]
    fn an_empty_uncompressed_payload_should_not_claim_a_valid_plc_state() {
        let mut dec = decoder();
        let mut pkt = uncompressed_packet(1);
        pkt.payload_len = 0;

        dec.capture(&pkt);

        assert!(!dec.plc_is_valid);
        assert_eq!(
            dec.decode_len, OPUS_FRAME_SAMPLES,
            "the frame is still filled"
        );
    }
}
