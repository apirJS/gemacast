//! Adapter: Network transport implementations for audio packet I/O.
//!
//! Production implementations of [`AudioPacketTransport`](crate::ports::transport::AudioPacketTransport):
//! - [`UdpTransport`] — UDP socket for WiFi streaming
//! - [`TcpTransport`] — TCP stream for ADB/USB streaming

use std::io::Read;
use std::net::{SocketAddr, TcpStream, UdpSocket};

use crate::ports::transport::AudioPacketTransport;

pub struct UdpTransport {
    pub socket: UdpSocket,
}

impl AudioPacketTransport for UdpTransport {
    fn receive_audio_packet(&mut self, buffer: &mut [u8]) -> std::io::Result<(usize, SocketAddr)> {
        self.socket.recv_from(buffer)
    }
}

pub struct TcpTransport {
    pub stream: TcpStream,
}

impl AudioPacketTransport for TcpTransport {
    fn receive_audio_packet(&mut self, buffer: &mut [u8]) -> std::io::Result<(usize, SocketAddr)> {
        let mut len_buf = [0u8; 4];

        self.stream.read_exact(&mut len_buf)?;

        let length = u32::from_be_bytes(len_buf) as usize;
        if length > buffer.len() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Packet too large for buffer",
            ));
        }

        self.stream.read_exact(&mut buffer[0..length])?;

        let addr = self.stream.peer_addr().unwrap_or_else(|_| {
            format!("127.0.0.1:{}", crate::network::Ports::ADB_AUDIO_TCP)
                .parse()
                .unwrap()
        });

        Ok((length, addr))
    }
}

// ---------------------------------------------------------------------------
// Enum dispatch transport (Strategy Pattern — static dispatch)
// ---------------------------------------------------------------------------

/// Static-dispatch audio transport using enum variants.
///
/// The compiler devirtualizes match arms into direct calls, eliminating
/// vtable pointer indirection on the receiver hot path (~4800 calls/sec).
///
/// This replaces the previous `Box<dyn AudioPacketTransport>` approach.
pub enum AudioTransport {
    Udp(UdpTransport),
    Tcp(TcpTransport),
}

impl AudioPacketTransport for AudioTransport {
    fn receive_audio_packet(
        &mut self,
        buffer: &mut [u8],
    ) -> std::io::Result<(usize, std::net::SocketAddr)> {
        match self {
            Self::Udp(t) => t.receive_audio_packet(buffer),
            Self::Tcp(t) => t.receive_audio_packet(buffer),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Helper: creates a connected loopback TcpStream pair and wraps the reader
    /// in a TcpTransport.
    fn make_tcp_pair() -> (TcpTransport, std::net::TcpStream) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let writer = std::net::TcpStream::connect(addr).unwrap();
        let (reader, _) = listener.accept().unwrap();

        // Set a short read timeout so tests don't hang on unexpected EOF
        reader
            .set_read_timeout(Some(std::time::Duration::from_secs(1)))
            .unwrap();

        (TcpTransport { stream: reader }, writer)
    }

    mod tcp_transport {
        use super::*;

        #[test]
        fn should_read_length_prefixed_packet() {
            let (mut transport, mut writer) = make_tcp_pair();

            let payload = [0xCA, 0xFE, 0xBA, 0xBE, 0x42];
            let len_bytes = (payload.len() as u32).to_be_bytes();
            writer.write_all(&len_bytes).unwrap();
            writer.write_all(&payload).unwrap();
            writer.flush().unwrap();

            let mut buffer = [0u8; 256];
            let (len, _addr) = transport.receive_audio_packet(&mut buffer).unwrap();
            assert_eq!(len, payload.len());
            assert_eq!(&buffer[..len], &payload);
        }

        #[test]
        fn should_reject_packet_larger_than_buffer() {
            let (mut transport, mut writer) = make_tcp_pair();

            // Claim the packet is 9999 bytes, but our buffer is only 64
            let len_bytes = (9999u32).to_be_bytes();
            writer.write_all(&len_bytes).unwrap();
            writer.flush().unwrap();

            let mut buffer = [0u8; 64];
            let result = transport.receive_audio_packet(&mut buffer);
            assert!(result.is_err(), "Should fail with oversized packet");
            assert_eq!(
                result.unwrap_err().kind(),
                std::io::ErrorKind::InvalidData,
                "Error kind should be InvalidData"
            );
        }

        #[test]
        fn should_return_error_on_incomplete_header() {
            let (mut transport, mut writer) = make_tcp_pair();

            // Write only 2 of the 4 header bytes, then close
            writer.write_all(&[0x00, 0x01]).unwrap();
            drop(writer);

            let mut buffer = [0u8; 256];
            let result = transport.receive_audio_packet(&mut buffer);
            assert!(result.is_err(), "Should fail when header is incomplete");
        }
    }
}
