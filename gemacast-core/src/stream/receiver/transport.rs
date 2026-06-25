//! Receiver-side transport orchestration.
//!
//! Creates and configures audio transport connections (UDP/TCP) for the
//! receiver. The `AudioTransport` enum adapter and underlying transport
//! structs live in [`crate::adapters::transport`].

use crate::adapters::transport::{AudioTransport, TcpTransport, UdpTransport};
use crate::domain::error::NetworkError;
use crate::network::Ports;
use std::net::{Ipv4Addr, SocketAddrV4};

pub fn create_udp_audio_transport(
    target_ip: Option<std::net::IpAddr>,
) -> Result<(UdpTransport, std::net::UdpSocket), NetworkError> {
    let addr = std::net::SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, Ports::AUDIO_UDP));
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
        std_socket
            .send_to(&[0u8], target_addr)
            .map_err(NetworkError::SendFailed)?;
    }

    let heartbeat_socket = std_socket
        .try_clone()
        .map_err(NetworkError::SocketCloneFailed)?;

    Ok((UdpTransport { socket: std_socket }, heartbeat_socket))
}

pub fn create_tcp_audio_transport(
    device_id: &crate::domain::types::DeviceId,
) -> Result<TcpTransport, NetworkError> {
    let adb_addr = format!("127.0.0.1:{}", Ports::ADB_AUDIO_TCP);
    let stream_addr: std::net::SocketAddr = adb_addr
        .parse()
        .expect("INTERNAL: ADB loopback address must be valid");

    let mut stream =
        std::net::TcpStream::connect_timeout(&stream_addr, std::time::Duration::from_millis(2500))
            .map_err(|source| NetworkError::TcpConnectFailed {
                addr: adb_addr.clone(),
                source,
            })?;

    use std::io::Write;

    let bytes = device_id.0.as_bytes();
    if stream.write_all(&[bytes.len() as u8]).is_err() || stream.write_all(bytes).is_err() {
        return Err(NetworkError::TcpConnectFailed {
            addr: adb_addr,
            source: std::io::Error::new(
                std::io::ErrorKind::ConnectionAborted,
                "Handshake write failed",
            ),
        });
    }

    let _ = stream.set_nodelay(true);
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_millis(2000)));

    Ok(TcpTransport { stream })
}

/// Creates the appropriate audio transport based on the connection mode.
///
/// Returns `(AudioTransport, Option<UdpSocket>)`:
/// - `AudioTransport`: Enum-dispatched transport (UDP or TCP)
/// - `Option<UdpSocket>`: Heartbeat socket (only for UDP/WiFi mode)
pub fn create_audio_transport(
    mode: crate::domain::types::ConnectionMode,
    target_ip: Option<std::net::IpAddr>,
    device_id: &crate::domain::types::DeviceId,
) -> Result<(AudioTransport, Option<std::net::UdpSocket>), NetworkError> {
    if mode == crate::domain::types::ConnectionMode::Adb {
        let t = create_tcp_audio_transport(device_id)?;
        return Ok((AudioTransport::Tcp(t), None));
    }

    let (udp, heartbeat_socket) = create_udp_audio_transport(target_ip)?;
    Ok((AudioTransport::Udp(udp), Some(heartbeat_socket)))
}
