//! Player-side transport orchestration.
//!
//! Creates and configures audio transport connections (UDP/TCP) for the
//! player. The `AudioTransport` enum adapter and underlying transport
//! structs live in [`crate::adapters::transport`].

use crate::adapters::transport::{AudioTransport, TcpTransport, UdpTransport};
use crate::control::SessionGeneration;
use crate::domain::error::{GemaCastError, NetworkError, ProtocolError};
use crate::network::Ports;
use std::net::{Ipv4Addr, SocketAddrV4};

use super::handshake::AdbAudioHandshake;

/// Creates the player-side UDP or ADB audio transport.
pub(crate) struct AudioTransportFactory;

impl AudioTransportFactory {
    pub(crate) fn create_udp(
        target_ip: Option<std::net::IpAddr>,
    ) -> Result<(UdpTransport, std::net::UdpSocket), NetworkError> {
        let addr =
            std::net::SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, Ports::AUDIO_UDP));
        let socket = socket2::Socket::new(
            socket2::Domain::IPV4,
            socket2::Type::DGRAM,
            Some(socket2::Protocol::UDP),
        )
        .map_err(|source| NetworkError::SocketBindFailed {
            addr: addr.to_string(),
            source,
        })?;

        socket
            .set_reuse_address(true)
            .map_err(|source| NetworkError::SocketOptionFailed {
                option: "reuse address",
                source,
            })?;
        #[cfg(not(windows))]
        socket
            .set_reuse_port(true)
            .map_err(|source| NetworkError::SocketOptionFailed {
                option: "reuse port",
                source,
            })?;

        socket
            .bind(&addr.into())
            .map_err(|source| NetworkError::SocketBindFailed {
                addr: addr.to_string(),
                source,
            })?;

        let _ = socket.set_recv_buffer_size(512 * 1024);

        let std_socket: std::net::UdpSocket = socket.into();

        let cloned_for_tos = std_socket
            .try_clone()
            .map_err(NetworkError::SocketCloneFailed)?;
        socket2::Socket::from(cloned_for_tos)
            .set_tos_v4(0xB8)
            .map_err(|source| NetworkError::SocketOptionFailed {
                option: "type of service",
                source,
            })?;

        std_socket
            .set_read_timeout(Some(std::time::Duration::from_millis(100)))
            .map_err(|source| NetworkError::SocketOptionFailed {
                option: "read timeout",
                source,
            })?;

        if let Some(target) = target_ip {
            let target_addr = std::net::SocketAddr::new(target, Ports::AUDIO_UDP);
            std_socket
                .send_to(&[0u8], target_addr)
                .map_err(NetworkError::SendFailed)?;
        }

        let heartbeat_socket = std_socket
            .try_clone()
            .map_err(NetworkError::SocketCloneFailed)?;

        Ok((UdpTransport { socket: std_socket }, heartbeat_socket))
    }

    pub(crate) fn create_tcp(
        device_id: &crate::domain::types::DeviceId,
        session_token: Option<&str>,
        session_generation: Option<SessionGeneration>,
    ) -> Result<TcpTransport, GemaCastError> {
        let adb_addr = format!("127.0.0.1:{}", Ports::ADB_AUDIO_TCP);
        let stream_addr: std::net::SocketAddr = adb_addr
            .parse()
            .expect("INTERNAL: ADB loopback address must be valid");

        let mut stream = std::net::TcpStream::connect_timeout(
            &stream_addr,
            std::time::Duration::from_millis(2500),
        )
        .map_err(|source| NetworkError::TcpConnectFailed {
            addr: adb_addr.clone(),
            source,
        })?;

        use std::io::Write;

        let token = session_token.ok_or(ProtocolError::MissingAdbSessionCredential {
            field: "session token",
        })?;
        let generation = session_generation.ok_or(ProtocolError::MissingAdbSessionCredential {
            field: "session generation",
        })?;
        let handshake = AdbAudioHandshake::encode(device_id, token, generation)?;
        stream
            .write_all(&handshake)
            .map_err(|source| NetworkError::TcpWriteFailed {
                addr: adb_addr,
                operation: "ADB audio handshake",
                source,
            })?;

        let _ = stream.set_nodelay(true);
        let _ = stream.set_read_timeout(Some(std::time::Duration::from_millis(2000)));

        Ok(TcpTransport { stream })
    }

    /// Creates the appropriate audio transport based on the connection mode.
    ///
    /// Returns `(AudioTransport, Option<UdpSocket>)`:
    /// - `AudioTransport`: Enum-dispatched transport (UDP or TCP)
    /// - `Option<UdpSocket>`: Heartbeat socket (only for UDP/WiFi mode)
    pub(crate) fn create(
        mode: crate::domain::types::ConnectionMode,
        target_ip: Option<std::net::IpAddr>,
        device_id: &crate::domain::types::DeviceId,
        session_token: Option<&str>,
        session_generation: Option<SessionGeneration>,
    ) -> Result<(AudioTransport, Option<std::net::UdpSocket>), GemaCastError> {
        if mode == crate::domain::types::ConnectionMode::Adb {
            let t = Self::create_tcp(device_id, session_token, session_generation)?;
            return Ok((AudioTransport::Tcp(t), None));
        }

        let (udp, heartbeat_socket) = Self::create_udp(target_ip)?;
        Ok((AudioTransport::Udp(udp), Some(heartbeat_socket)))
    }
}
