use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;

/// Length-prefixed TCP audio framer for ADB transport.
///
/// Batches multiple audio packets into a single TCP write by prepending
/// each packet with a 4-byte big-endian length header. The receiver side
/// (`TcpTransport`) reads the matching `read_exact` framing.
pub struct TcpAudioFramer {
    batch: Vec<u8>,
}

impl TcpAudioFramer {
    /// Creates a new framer with a pre-allocated batch buffer.
    pub fn new() -> Self {
        Self {
            batch: Vec::with_capacity(65536),
        }
    }

    /// Appends one audio packet with a 4-byte big-endian length prefix.
    pub fn append_packet(&mut self, payload: &[u8]) {
        let len = payload.len() as u32;
        self.batch.extend_from_slice(&len.to_be_bytes());
        self.batch.extend_from_slice(payload);
    }

    /// Returns true if the batch buffer contains pending data.
    pub fn has_pending(&self) -> bool {
        !self.batch.is_empty()
    }

    /// Writes all pending packets to the socket and flushes.
    /// Returns `Err` if the write fails (connection dropped).
    pub async fn flush(&mut self, socket: &mut TcpStream) -> std::io::Result<()> {
        if self.batch.is_empty() {
            return Ok(());
        }
        socket.write_all(&self.batch).await?;
        socket.flush().await?;
        self.batch.clear();
        Ok(())
    }

    /// Discards all pending data without writing.
    pub fn clear(&mut self) {
        self.batch.clear();
    }
}

impl Default for TcpAudioFramer {
    fn default() -> Self {
        Self::new()
    }
}
