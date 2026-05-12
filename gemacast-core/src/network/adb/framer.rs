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
