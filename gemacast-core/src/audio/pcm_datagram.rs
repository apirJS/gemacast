use super::{FORMAT_UNCOMPRESSED, OPUS_FRAME_SAMPLES};
use crate::domain::error::ProtocolError;

/// V1 preserves the existing 10 ms frame sequence, with four 2.5 ms PCM slices.
pub(crate) struct PcmDatagram;

impl PcmDatagram {
    pub const FORMAT: u8 = 3;
    pub const CHUNKS: usize = 4;
    pub const PAYLOAD: usize = OPUS_FRAME_SAMPLES * size_of::<f32>() / Self::CHUNKS;
    pub const HEADER: usize = 18;
    pub const SIZE: usize = Self::HEADER + Self::PAYLOAD;

    pub fn encode(frame: &[u8], generation: u64, chunk: usize, out: &mut [u8; Self::SIZE]) {
        assert_eq!(frame.len(), 9 + Self::PAYLOAD * Self::CHUNKS);
        assert_eq!(frame[8], FORMAT_UNCOMPRESSED);
        assert!(chunk < Self::CHUNKS);
        out[..8].copy_from_slice(&frame[..8]);
        out[8] = Self::FORMAT;
        out[9..17].copy_from_slice(&generation.to_be_bytes());
        out[17] = chunk as u8;
        let offset = 9 + chunk * Self::PAYLOAD;
        out[Self::HEADER..].copy_from_slice(&frame[offset..offset + Self::PAYLOAD]);
    }

    pub fn decode(bytes: &[u8]) -> Result<PcmChunk<'_>, ProtocolError> {
        if bytes.len() != Self::SIZE {
            return Err(ProtocolError::InvalidPcmDatagram {
                reason: "wrong chunk size",
            });
        }
        if bytes[8] != Self::FORMAT || bytes[17] as usize >= Self::CHUNKS {
            return Err(ProtocolError::InvalidPcmDatagram {
                reason: "unknown format or chunk index",
            });
        }
        Ok(PcmChunk {
            sequence: u64::from_be_bytes(bytes[..8].try_into().unwrap()),
            generation: u64::from_be_bytes(bytes[9..17].try_into().unwrap()),
            index: bytes[17] as usize,
            payload: &bytes[Self::HEADER..],
        })
    }
}

pub(crate) struct PcmChunk<'a> {
    pub sequence: u64,
    pub generation: u64,
    pub index: usize,
    pub payload: &'a [u8],
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_should_preserve_every_pcm_byte_without_exceeding_the_datagram_budget() {
        let mut frame = vec![0; 9 + PcmDatagram::PAYLOAD * 4];
        frame[..8].copy_from_slice(&42u64.to_be_bytes());
        frame[8] = FORMAT_UNCOMPRESSED;
        for (i, byte) in frame[9..].iter_mut().enumerate() {
            *byte = i as u8;
        }
        let mut recovered = Vec::new();
        for index in 0..4 {
            let mut packet = [0; PcmDatagram::SIZE];
            PcmDatagram::encode(&frame, 123, index, &mut packet);
            assert!(packet.len() <= 1200);
            let chunk = PcmDatagram::decode(&packet).unwrap();
            assert_eq!(
                (chunk.sequence, chunk.generation, chunk.index),
                (42, 123, index)
            );
            recovered.extend_from_slice(chunk.payload);
        }
        assert_eq!(recovered, frame[9..]);
    }

    #[test]
    fn malformed_chunks_should_be_rejected_before_their_header_is_used() {
        for len in [0, 8, 18, PcmDatagram::SIZE - 1, PcmDatagram::SIZE + 1] {
            assert!(PcmDatagram::decode(&vec![0; len]).is_err());
        }
        let mut packet = [0; PcmDatagram::SIZE];
        packet[8] = PcmDatagram::FORMAT;
        packet[17] = 4;
        assert!(PcmDatagram::decode(&packet).is_err());
    }
}
