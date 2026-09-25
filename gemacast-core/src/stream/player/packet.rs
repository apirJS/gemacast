use crate::audio::{FORMAT_FLAG_SIZE, FORMAT_SILENCE, FORMAT_UNCOMPRESSED, SEQ_NUM_SIZE};
use crate::domain::error::ProtocolError;
use crate::jitter::RawPacket;
use crate::jitter::types::MAX_PACKET_PAYLOAD;
use std::time::Instant;

/// Decodes audio datagrams and estimates their receive-side activity level.
pub(crate) struct AudioPacketDecoder;

impl AudioPacketDecoder {
    pub(crate) fn decode(buffer: &[u8], len: usize) -> Result<RawPacket, ProtocolError> {
        let header_len = SEQ_NUM_SIZE + FORMAT_FLAG_SIZE;
        if len < header_len {
            return Err(ProtocolError::PacketTooShort {
                got: len,
                min: header_len,
            });
        }
        if len > buffer.len() {
            return Err(ProtocolError::PacketLengthExceedsBuffer {
                declared: len,
                available: buffer.len(),
            });
        }

        let seq_bytes: [u8; 8] = buffer[..SEQ_NUM_SIZE]
            .try_into()
            .expect("packet header length was validated");
        let seq_num = u64::from_be_bytes(seq_bytes);

        let format_flag = buffer[SEQ_NUM_SIZE];
        if !matches!(
            format_flag,
            crate::audio::FORMAT_OPUS | FORMAT_UNCOMPRESSED | FORMAT_SILENCE
        ) {
            return Err(ProtocolError::UnsupportedAudioFormat(format_flag));
        }
        let is_uncompressed = format_flag == FORMAT_UNCOMPRESSED;
        let is_silence = format_flag == FORMAT_SILENCE;

        let payload_len = len - header_len;
        if payload_len > MAX_PACKET_PAYLOAD {
            return Err(ProtocolError::PacketPayloadTooLarge {
                got: payload_len,
                max: MAX_PACKET_PAYLOAD,
            });
        }
        let mut payload_data = [0u8; MAX_PACKET_PAYLOAD];
        payload_data[..payload_len].copy_from_slice(&buffer[header_len..len]);

        Ok(RawPacket {
            seq_num,
            payload_data,
            payload_len,
            arrival_time: Instant::now(),
            is_uncompressed,
            is_silence,
        })
    }

    pub(crate) fn estimate_rms(rms_data: &[u8], is_silence: bool, is_uncompressed: bool) -> f32 {
        if is_silence {
            return 0.0;
        }

        if is_uncompressed {
            let mut sum_sq = 0.0f32;
            let mut count = 0;
            for chunk in rms_data.chunks_exact(4) {
                let sample = f32::from_ne_bytes(chunk.try_into().expect("chunk is four bytes"));
                sum_sq += sample * sample;
                count += 1;
            }
            if count > 0 {
                (sum_sq / count as f32).sqrt()
            } else {
                0.0
            }
        } else {
            let ms_per_frame = (crate::audio::OPUS_FRAME_SAMPLES as f32
                / crate::audio::OPUS_CHANNELS as f32
                / crate::audio::OPUS_SAMPLE_RATE as f32)
                * 1000.0;
            let bitrate_bytes_per_sec = crate::audio::OPUS_BITRATE as f32 / 8.0;
            let typical_max = bitrate_bytes_per_sec * ms_per_frame / 1000.0;
            (rms_data.len() as f32 / typical_max).min(1.0).sqrt()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::{
        FORMAT_FLAG_SIZE, FORMAT_OPUS, FORMAT_SILENCE, FORMAT_UNCOMPRESSED, SEQ_NUM_SIZE,
    };

    fn build_raw(seq: u64, format_flag: u8, payload: &[u8]) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(&seq.to_be_bytes());
        buf.push(format_flag);
        buf.extend_from_slice(payload);
        buf
    }

    #[test]
    fn should_classify_a_packet_shorter_than_the_wire_header() {
        let buf = [0u8; 5];
        assert!(matches!(
            AudioPacketDecoder::decode(&buf, 5),
            Err(ProtocolError::PacketTooShort { got: 5, .. })
        ));
    }

    #[test]
    fn should_classify_a_declared_length_beyond_the_receive_buffer() {
        let buf = [0u8; SEQ_NUM_SIZE + FORMAT_FLAG_SIZE];

        assert!(matches!(
            AudioPacketDecoder::decode(&buf, buf.len() + 1),
            Err(ProtocolError::PacketLengthExceedsBuffer { .. })
        ));
    }

    #[test]
    fn should_classify_a_payload_larger_than_the_jitter_packet_capacity() {
        let buf = vec![0u8; SEQ_NUM_SIZE + FORMAT_FLAG_SIZE + MAX_PACKET_PAYLOAD + 1];

        assert!(matches!(
            AudioPacketDecoder::decode(&buf, buf.len()),
            Err(ProtocolError::PacketPayloadTooLarge { .. })
        ));
    }

    #[test]
    fn should_decode_the_opus_format_flag() {
        let buf = build_raw(42, FORMAT_OPUS, &[1, 2, 3]);
        let pkt = AudioPacketDecoder::decode(&buf, buf.len()).unwrap();
        assert_eq!(pkt.seq_num, 42);
        assert!(!pkt.is_uncompressed);
        assert!(!pkt.is_silence);
        assert_eq!(pkt.payload_len, 3);
    }

    #[test]
    fn should_decode_the_uncompressed_format_flag() {
        let buf = build_raw(10, FORMAT_UNCOMPRESSED, &[0; 16]);
        let pkt = AudioPacketDecoder::decode(&buf, buf.len()).unwrap();
        assert!(pkt.is_uncompressed);
        assert!(!pkt.is_silence);
    }

    #[test]
    fn should_decode_the_silence_format_flag() {
        let buf = build_raw(99, FORMAT_SILENCE, &[]);
        let pkt = AudioPacketDecoder::decode(&buf, buf.len()).unwrap();
        assert!(pkt.is_silence);
        assert_eq!(pkt.payload_len, 0);
    }

    #[test]
    fn should_accept_a_header_without_a_payload() {
        let buf = build_raw(0, FORMAT_OPUS, &[]);
        let pkt = AudioPacketDecoder::decode(&buf, SEQ_NUM_SIZE + FORMAT_FLAG_SIZE).unwrap();
        assert_eq!(pkt.seq_num, 0);
        assert_eq!(pkt.payload_len, 0);
    }

    #[test]
    fn should_report_zero_rms_for_silence() {
        assert_eq!(
            AudioPacketDecoder::estimate_rms(&[1, 2, 3], true, false),
            0.0
        );
    }

    #[test]
    fn should_compute_rms_from_uncompressed_samples() {
        let val: f32 = 0.5;
        let bytes = val.to_ne_bytes();
        let mut data = Vec::new();
        for _ in 0..4 {
            data.extend_from_slice(&bytes);
        }
        let rms = AudioPacketDecoder::estimate_rms(&data, false, true);
        assert!((rms - 0.5).abs() < 0.001);
    }
}
