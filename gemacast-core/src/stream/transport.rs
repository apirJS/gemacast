//! Audio packet transport abstractions.
//!
//! [`AudioPacketTransport`] provides a unified interface for receiving audio
//! packets regardless of the underlying transport (UDP or TCP).

use std::io::Read;
use std::net::{SocketAddr, TcpStream, UdpSocket};

/// Trait for receiving audio packets from a network transport.
///
/// Implementors abstract the differences between UDP (WiFi/USB tethering)
/// and TCP (ADB tunnel) so the receiver pipeline can be transport-agnostic.
pub trait AudioPacketTransport: Send {
    /// Receives a single audio packet into `buffer`, returning the number
    /// of bytes read and the remote sender's socket address.
    fn receive_audio_packet(&mut self, buffer: &mut [u8]) -> std::io::Result<(usize, SocketAddr)>;
}

/// UDP transport — receives raw datagrams for WiFi and USB tethering modes.
pub struct UdpTransport {
    pub socket: UdpSocket,
}

impl AudioPacketTransport for UdpTransport {
    fn receive_audio_packet(&mut self, buffer: &mut [u8]) -> std::io::Result<(usize, SocketAddr)> {
        self.socket.recv_from(buffer)
    }
}

/// TCP transport — receives length-prefixed frames for ADB tunnel mode.
///
/// Each packet is preceded by a 4-byte big-endian length header, matching
/// the framing produced by [`TcpAudioFramer`].
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

        let addr = self
            .stream
            .peer_addr()
            .unwrap_or_else(|_| "127.0.0.1:55557".parse().unwrap());

        Ok((length, addr))
    }
}
