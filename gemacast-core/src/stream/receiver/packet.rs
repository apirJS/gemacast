use crate::audio::{FORMAT_FLAG_SIZE, FORMAT_SILENCE, FORMAT_UNCOMPRESSED, SEQ_NUM_SIZE};
use crate::jitter::RawPacket;
use crate::jitter::types::MAX_PACKET_PAYLOAD;
use std::time::Instant;

pub fn parse_packet(buffer: &[u8], len: usize) -> Option<RawPacket> {
    if len < SEQ_NUM_SIZE + FORMAT_FLAG_SIZE {
        return None;
    }

    let seq_bytes: [u8; 8] = buffer[..SEQ_NUM_SIZE].try_into().unwrap();
    let seq_num = u64::from_be_bytes(seq_bytes);

    let format_flag = buffer[SEQ_NUM_SIZE];
    let is_uncompressed = format_flag == FORMAT_UNCOMPRESSED;
    let is_silence = format_flag == FORMAT_SILENCE;

    let payload_len = len - (SEQ_NUM_SIZE + FORMAT_FLAG_SIZE);
    let mut payload_data = [0u8; MAX_PACKET_PAYLOAD];
    if payload_len > 0 {
        let copy_len = payload_len.min(MAX_PACKET_PAYLOAD);
        payload_data[..copy_len].copy_from_slice(
            &buffer[SEQ_NUM_SIZE + FORMAT_FLAG_SIZE..SEQ_NUM_SIZE + FORMAT_FLAG_SIZE + copy_len],
        );
    }

    Some(RawPacket {
        seq_num,
        payload_data,
        payload_len,
        arrival_time: Instant::now(),
        is_uncompressed,
        is_silence,
    })
}

pub fn compute_rms(rms_data: &[u8], is_silence: bool, is_uncompressed: bool) -> f32 {
    if is_silence {
        return 0.0;
    }

    if is_uncompressed {
        let mut sum_sq = 0.0f32;
        let mut count = 0;
        for chunk in rms_data.chunks_exact(4) {
            let f = f32::from_ne_bytes(chunk.try_into().unwrap());
            sum_sq += f * f;
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
    fn parse_packet_should_return_none_when_buffer_too_short() {
        let buf = [0u8; 5];
        assert!(parse_packet(&buf, 5).is_none());
    }

    #[test]
    fn parse_packet_should_decode_opus_format_flag() {
        let buf = build_raw(42, FORMAT_OPUS, &[1, 2, 3]);
        let pkt = parse_packet(&buf, buf.len()).unwrap();
        assert_eq!(pkt.seq_num, 42);
        assert!(!pkt.is_uncompressed);
        assert!(!pkt.is_silence);
        assert_eq!(pkt.payload_len, 3);
    }

    #[test]
    fn parse_packet_should_decode_uncompressed_format_flag() {
        let buf = build_raw(10, FORMAT_UNCOMPRESSED, &[0; 16]);
        let pkt = parse_packet(&buf, buf.len()).unwrap();
        assert!(pkt.is_uncompressed);
        assert!(!pkt.is_silence);
    }

    #[test]
    fn parse_packet_should_decode_silence_format_flag() {
        let buf = build_raw(99, FORMAT_SILENCE, &[]);
        let pkt = parse_packet(&buf, buf.len()).unwrap();
        assert!(pkt.is_silence);
        assert_eq!(pkt.payload_len, 0);
    }

    #[test]
    fn parse_packet_should_handle_minimum_header_only() {
        let buf = build_raw(0, FORMAT_OPUS, &[]);
        let pkt = parse_packet(&buf, SEQ_NUM_SIZE + FORMAT_FLAG_SIZE).unwrap();
        assert_eq!(pkt.seq_num, 0);
        assert_eq!(pkt.payload_len, 0);
    }

    #[test]
    fn compute_rms_should_return_zero_for_silence() {
        assert_eq!(compute_rms(&[1, 2, 3], true, false), 0.0);
    }

    #[test]
    fn compute_rms_should_compute_uncompressed_correctly() {
        let val: f32 = 0.5;
        let bytes = val.to_ne_bytes();
        let mut data = Vec::new();
        for _ in 0..4 {
            data.extend_from_slice(&bytes);
        }
        let rms = compute_rms(&data, false, true);
        assert!((rms - 0.5).abs() < 0.001);
    }
}
