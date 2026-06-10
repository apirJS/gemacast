use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;

pub struct TcpAudioFramer {
    batch: Vec<u8>,
}

impl TcpAudioFramer {
    pub fn new() -> Self {
        Self {
            batch: Vec::with_capacity(65536),
        }
    }

    pub fn append_packet(&mut self, payload: &[u8]) {
        let len = payload.len() as u32;
        self.batch.extend_from_slice(&len.to_be_bytes());
        self.batch.extend_from_slice(payload);
    }

    pub fn has_pending(&self) -> bool {
        !self.batch.is_empty()
    }

    pub async fn flush(&mut self, socket: &mut TcpStream) -> std::io::Result<()> {
        if self.batch.is_empty() {
            return Ok(());
        }
        socket.write_all(&self.batch).await?;
        socket.flush().await?;
        self.batch.clear();
        Ok(())
    }

    pub fn clear(&mut self) {
        self.batch.clear();
    }
}

impl Default for TcpAudioFramer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod tcp_audio_framer {
        use super::*;

        #[test]
        fn new_should_have_no_pending_data() {
            let framer = TcpAudioFramer::new();
            assert!(
                !framer.has_pending(),
                "Freshly created framer should have no pending data"
            );
        }

        #[test]
        fn append_packet_should_mark_as_pending() {
            let mut framer = TcpAudioFramer::new();
            framer.append_packet(&[0xAA, 0xBB]);
            assert!(
                framer.has_pending(),
                "Framer should have pending data after append"
            );
        }

        #[test]
        fn clear_should_reset_pending_state() {
            let mut framer = TcpAudioFramer::new();
            framer.append_packet(&[1, 2, 3]);
            framer.clear();
            assert!(
                !framer.has_pending(),
                "Framer should have no pending data after clear"
            );
        }

        #[test]
        fn single_packet_should_produce_length_prefixed_frame() {
            let mut framer = TcpAudioFramer::new();
            let payload = [0xDE, 0xAD, 0xBE, 0xEF];
            framer.append_packet(&payload);

            // Expected: [4-byte big-endian length][payload]
            let expected_len = (payload.len() as u32).to_be_bytes();
            let batch = &framer.batch;
            assert_eq!(
                &batch[..4],
                &expected_len,
                "First 4 bytes should be big-endian length"
            );
            assert_eq!(
                &batch[4..],
                &payload,
                "Remaining bytes should be the payload"
            );
        }

        #[test]
        fn multiple_packets_should_batch_contiguously() {
            let mut framer = TcpAudioFramer::new();
            let p1 = [0x01, 0x02];
            let p2 = [0x03, 0x04, 0x05];
            framer.append_packet(&p1);
            framer.append_packet(&p2);

            let batch = &framer.batch;
            // First packet: 4-byte header + 2-byte payload = 6 bytes
            // Second packet: 4-byte header + 3-byte payload = 7 bytes
            assert_eq!(
                batch.len(),
                6 + 7,
                "Batch should contain both framed packets"
            );

            // Verify first packet frame
            let len1 = u32::from_be_bytes(batch[0..4].try_into().unwrap());
            assert_eq!(len1, 2);
            assert_eq!(&batch[4..6], &p1);

            // Verify second packet frame
            let len2 = u32::from_be_bytes(batch[6..10].try_into().unwrap());
            assert_eq!(len2, 3);
            assert_eq!(&batch[10..13], &p2);
        }
    }
}
