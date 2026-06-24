use crate::control::messages::ControlMessage;
use crate::domain::error::{GemaCastError, NetworkError};
use crate::network::Ports;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::sync::mpsc;

pub struct PresenceListener {
    pub socket: Arc<UdpSocket>,
    incoming_message_tx: mpsc::Sender<(ControlMessage, std::net::SocketAddr)>,
}

impl PresenceListener {
    pub async fn new(
        incoming_message_tx: mpsc::Sender<(ControlMessage, std::net::SocketAddr)>,
    ) -> Result<Self, GemaCastError> {
        tracing::info!("Initializing PresenceListener");
        let addr = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, Ports::DISCOVERY);
        let socket = socket2::Socket::new(
            socket2::Domain::IPV4,
            socket2::Type::DGRAM,
            Some(socket2::Protocol::UDP),
        )
        .map_err(|e| NetworkError::SocketBindFailed {
            addr: addr.to_string(),
            source: e,
        })?;

        socket.set_reuse_address(true).ok();
        #[cfg(not(windows))]
        socket.set_reuse_port(true).ok();

        socket
            .bind(&addr.into())
            .map_err(|e| NetworkError::SocketBindFailed {
                addr: addr.to_string(),
                source: e,
            })?;

        socket.set_nonblocking(true).ok();
        let socket =
            UdpSocket::from_std(socket.into()).map_err(|e| NetworkError::SocketBindFailed {
                addr: addr.to_string(),
                source: e,
            })?;

        let multicast_ip = Ipv4Addr::new(224, 0, 0, 124);

        let local_bind_ip = match crate::network::get_local_ip() {
            Ok(std::net::IpAddr::V4(v4)) => v4,
            _ => Ipv4Addr::UNSPECIFIED,
        };

        if !local_bind_ip.is_link_local()
            && let Err(_e) = socket.join_multicast_v4(multicast_ip, local_bind_ip)
        {}

        Ok(Self {
            incoming_message_tx,
            socket: Arc::new(socket),
        })
    }

    pub async fn run_receive_loop(&self) -> Result<(), GemaCastError> {
        tracing::info!("Starting UDP presence receive loop");
        let mut buff = vec![0u8; 2048];

        loop {
            let (len, remote_addr) = match self.socket.recv_from(&mut buff).await {
                Ok(result) => result,
                Err(_e) => {
                    continue;
                }
            };

            let packet_data = &buff[..len];

            match serde_json::from_slice::<ControlMessage>(packet_data) {
                Ok(message) => {
                    if let Err(_e) = self.incoming_message_tx.send((message, remote_addr)).await {
                        break;
                    }
                }
                Err(_e) => {
                    continue;
                }
            }
        }

        Ok(())
    }
}
