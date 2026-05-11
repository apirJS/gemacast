use crate::error::NetworkError;
use crate::stream::transport::{TcpTransport, UdpTransport};
use crate::network::Ports;
use std::net::{Ipv4Addr, SocketAddrV4};

pub fn setup_udp_transport(
    target_ip: Option<std::net::IpAddr>,
) -> Result<(UdpTransport, std::net::UdpSocket), NetworkError> {
    let addr = std::net::SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, Ports::AUDIO_UDP));
    let socket = socket2::Socket::new(
        socket2::Domain::IPV4,
        socket2::Type::DGRAM,
        Some(socket2::Protocol::UDP),
    )
    .map_err(|source| NetworkError::BindFailed {
        addr: addr.to_string(),
        source,
    })?;

    socket
        .set_reuse_address(true)
        .map_err(NetworkError::SetReuseAddressFailed)?;
    #[cfg(not(windows))]
    socket
        .set_reuse_port(true)
        .map_err(NetworkError::SetReusePortFailed)?;

    socket
        .bind(&addr.into())
        .map_err(|source| NetworkError::BindFailed {
            addr: addr.to_string(),
            source,
        })?;

    let std_socket: std::net::UdpSocket = socket.into();

    let cloned_for_tos = std_socket
        .try_clone()
        .map_err(NetworkError::SocketCloneFailed)?;
    socket2::Socket::from(cloned_for_tos)
        .set_tos(0xB8)
        .map_err(NetworkError::SetTosFailed)?;

    std_socket
        .set_read_timeout(Some(std::time::Duration::from_millis(100)))
        .map_err(NetworkError::SetReadTimeoutFailed)?;

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

pub fn setup_tcp_transport() -> Result<TcpTransport, NetworkError> {
    let adb_addr = format!("127.0.0.1:{}", Ports::ADB_AUDIO_TCP);
    let stream_addr: std::net::SocketAddr = adb_addr.parse().unwrap();

    let stream = std::net::TcpStream::connect_timeout(
        &stream_addr,
        std::time::Duration::from_millis(2500),
    )
    .map_err(|source| NetworkError::TcpConnectFailed {
        addr: adb_addr,
        source,
    })?;

    let _ = stream.set_nodelay(true);
    Ok(TcpTransport { stream })
}

pub fn setup_transport(
    mode: crate::types::ConnectionMode,
    target_ip: Option<std::net::IpAddr>,
) -> Result<
    (
        Box<dyn crate::stream::transport::AudioTransport>,
        Option<std::net::UdpSocket>,
    ),
    NetworkError,
> {
    if mode == crate::types::ConnectionMode::Adb {
        let t = setup_tcp_transport()?;
        return Ok((Box::new(t), None));
    }

    let (udp, heartbeat_socket) = setup_udp_transport(target_ip)?;
    Ok((Box::new(udp), Some(heartbeat_socket)))
}
