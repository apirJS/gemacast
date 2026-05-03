use crate::audio::{
    FORMAT_OPUS, FORMAT_SILENCE, FORMAT_UNCOMPRESSED,
    OPUS_FRAME_SAMPLES,
};
use opus::Encoder;

pub enum EncodeResult {
    Encoded,
    Skipped,
}

pub fn encode_frame(
    frame: &[f32],
    encoder: &mut Encoder,
    current_bitrate: Option<i32>,
    seq_num: u64,
    opus_output: &mut [u8],
    packet_buf: &mut Vec<u8>,
) -> EncodeResult {
    let mut sum_sq = 0.0f32;
    for sample in frame {
        sum_sq += sample * sample;
    }
    let rms = (sum_sq / OPUS_FRAME_SAMPLES as f32).sqrt();

    let is_silence = rms < 0.0001;
    let is_uncompressed = current_bitrate.is_none();

    let format_flag = if is_silence {
        FORMAT_SILENCE
    } else if is_uncompressed {
        FORMAT_UNCOMPRESSED
    } else {
        FORMAT_OPUS
    };

    let payload_bytes: &[u8] = if is_silence {
        &[]
    } else if is_uncompressed {
        // Safety: frame is a properly aligned &[f32].
        unsafe {
            std::slice::from_raw_parts(
                frame.as_ptr() as *const u8,
                OPUS_FRAME_SAMPLES * std::mem::size_of::<f32>(),
            )
        }
    } else {
        let encoded_len = match encoder.encode_float(frame, opus_output) {
            Ok(e) => e,
            Err(_) => return EncodeResult::Skipped,
        };
        &opus_output[..encoded_len]
    };

    packet_buf.clear();
    packet_buf.extend_from_slice(&seq_num.to_be_bytes());
    packet_buf.push(format_flag);
    packet_buf.extend_from_slice(payload_bytes);

    EncodeResult::Encoded
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::{
        FORMAT_OPUS, FORMAT_SILENCE, FORMAT_UNCOMPRESSED, MAX_OPUS_PACKET_SIZE,
        OPUS_FRAME_SAMPLES, SEQ_NUM_SIZE, FORMAT_FLAG_SIZE,
    };

    fn make_encoder() -> Encoder {
        crate::audio::create_opus_encoder().unwrap()
    }

    #[test]
    fn encode_frame_should_produce_silence_flag_for_quiet_audio() {
        let mut encoder = make_encoder();
        let frame = vec![0.0f32; OPUS_FRAME_SAMPLES];
        let mut opus_out = vec![0u8; MAX_OPUS_PACKET_SIZE];
        let mut packet = Vec::new();

        let result = encode_frame(&frame, &mut encoder, Some(128_000), 5, &mut opus_out, &mut packet);
        assert!(matches!(result, EncodeResult::Encoded));
        assert_eq!(packet[SEQ_NUM_SIZE], FORMAT_SILENCE);
        assert_eq!(packet.len(), SEQ_NUM_SIZE + FORMAT_FLAG_SIZE); // no payload
    }

    #[test]
    fn encode_frame_should_produce_uncompressed_flag_when_no_bitrate() {
        let mut encoder = make_encoder();
        let mut frame = vec![0.0f32; OPUS_FRAME_SAMPLES];
        frame[0] = 0.5; // non-silent
        frame[1] = 0.5;
        let mut opus_out = vec![0u8; MAX_OPUS_PACKET_SIZE];
        let mut packet = Vec::new();

        let result = encode_frame(&frame, &mut encoder, None, 10, &mut opus_out, &mut packet);
        assert!(matches!(result, EncodeResult::Encoded));
        assert_eq!(packet[SEQ_NUM_SIZE], FORMAT_UNCOMPRESSED);
    }

    #[test]
    fn encode_frame_should_produce_opus_flag_for_normal_audio() {
        let mut encoder = make_encoder();
        let frame = vec![0.1f32; OPUS_FRAME_SAMPLES];
        let mut opus_out = vec![0u8; MAX_OPUS_PACKET_SIZE];
        let mut packet = Vec::new();

        let result = encode_frame(&frame, &mut encoder, Some(128_000), 7, &mut opus_out, &mut packet);
        assert!(matches!(result, EncodeResult::Encoded));
        assert_eq!(packet[SEQ_NUM_SIZE], FORMAT_OPUS);
        assert!(packet.len() > SEQ_NUM_SIZE + FORMAT_FLAG_SIZE); // has opus payload
    }

    #[test]
    fn encode_frame_should_prepend_sequence_number() {
        let mut encoder = make_encoder();
        let frame = vec![0.0f32; OPUS_FRAME_SAMPLES];
        let mut opus_out = vec![0u8; MAX_OPUS_PACKET_SIZE];
        let mut packet = Vec::new();

        encode_frame(&frame, &mut encoder, Some(128_000), 0xDEAD, &mut opus_out, &mut packet);
        let seq = u64::from_be_bytes(packet[..8].try_into().unwrap());
        assert_eq!(seq, 0xDEAD);
    }
}
