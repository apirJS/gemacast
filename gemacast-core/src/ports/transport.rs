//! Port: Audio packet transport abstraction.
//!
//! Defines the [`AudioPacketTransport`] trait that decouples the receiver's
//! packet receive loop from the concrete transport mechanism (UDP vs TCP).
//!
//! # Strategy Pattern
//!
//! `AudioPacketTransport` is the Strategy interface for network transport.
//! The receiver selects the strategy at startup based on [`ConnectionMode`](crate::domain::types::ConnectionMode):
//!
//! | Strategy | Transport | Use Case |
//! |---|---|---|
//! | [`UdpTransport`](crate::adapters::transport::UdpTransport) | UDP socket | WiFi streaming |
//! | [`TcpTransport`](crate::adapters::transport::TcpTransport) | TCP stream | ADB/USB streaming |
//! | `MockTransport` | In-memory | Tests |

use std::net::SocketAddr;

/// Receives audio packets from the network.
///
/// Called ~4800 times/sec at 5ms frame intervals on the receiver's
/// dedicated network thread. This is the **hottest I/O path** in the system —
/// static dispatch (via generics or enum dispatch) is critical here.
///
/// # Contract
///
/// - Implementations MUST be blocking with a reasonable timeout (100ms–2s).
/// - `buffer` is pre-allocated by the caller; implementations write into it.
/// - Returns `(bytes_read, sender_address)` on success.
/// - Returns `ErrorKind::WouldBlock` / `ErrorKind::TimedOut` on timeout (not fatal).
/// - Returns `ErrorKind::UnexpectedEof` / `ErrorKind::ConnectionReset` on disconnect (fatal).
pub trait AudioPacketTransport: Send {
    fn receive_audio_packet(&mut self, buffer: &mut [u8]) -> std::io::Result<(usize, SocketAddr)>;
}
