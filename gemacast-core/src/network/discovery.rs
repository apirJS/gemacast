use crate::error::{GemaCastError, NetworkError};
use crate::types::ControlMessage;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, oneshot};
use tokio::time::sleep;

use super::DISCOVERY_PORT;

pub struct DiscoveryListener {
    pub socket: Arc<UdpSocket>,
    discovery_tx: mpsc::Sender<(ControlMessage, std::net::SocketAddr)>,
}

pub struct DiscoveryListenerHandles {
    pub listener: DiscoveryListener,
    pub discovery_rx: mpsc::Receiver<(ControlMessage, std::net::SocketAddr)>,
}

impl DiscoveryListener {
    #[expect(clippy::new_ret_no_self, reason = "returns a handles bundle by design")]
    pub async fn new() -> Result<DiscoveryListenerHandles, GemaCastError> {
        let (discovery_tx, discovery_rx) =
            mpsc::channel::<(ControlMessage, std::net::SocketAddr)>(8);

        let addr = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, DISCOVERY_PORT);

        // Use socket2 to set SO_REUSEADDR and SO_REUSEPORT before binding
        let socket = socket2::Socket::new(
            socket2::Domain::IPV4,
            socket2::Type::DGRAM,
            Some(socket2::Protocol::UDP),
        )
        .map_err(|e| NetworkError::BindFailed {
            addr: addr.to_string(),
            source: e,
        })?;

        socket.set_reuse_address(true).ok();
        #[cfg(not(windows))]
        socket.set_reuse_port(true).ok();

        socket
            .bind(&addr.into())
            .map_err(|e| NetworkError::BindFailed {
                addr: addr.to_string(),
                source: e,
            })?;

        socket.set_nonblocking(true).ok();
        let socket = UdpSocket::from_std(socket.into()).map_err(|e| NetworkError::BindFailed {
            addr: addr.to_string(),
            source: e,
        })?;

        let multicast_ip = Ipv4Addr::new(224, 0, 0, 124);

        let local_bind_ip = match super::get_local_ip() {
            Ok(std::net::IpAddr::V4(v4)) => v4,
            _ => Ipv4Addr::UNSPECIFIED,
        };

        match socket.join_multicast_v4(multicast_ip, local_bind_ip) {
            Ok(_) => {}
            Err(e) => {
                eprintln!(
                    "Warning: Failed to join multicast on {}: {}. Continuing with broadcast.",
                    local_bind_ip, e
                );
            }
        }

        Ok(DiscoveryListenerHandles {
            listener: DiscoveryListener {
                discovery_tx,
                socket: Arc::new(socket),
            },
            discovery_rx,
        })
    }

    pub async fn start(&self) -> Result<(), GemaCastError> {
        let mut buff = vec![0u8; 2048];

        loop {
            let (len, remote_addr) = match self.socket.recv_from(&mut buff).await {
                Ok(result) => result,
                Err(e) => {
                    eprintln!(
                        "Error receiving UDP packet: {}",
                        NetworkError::RecvFailed(e)
                    );
                    continue;
                }
            };

            let packet_data = &buff[..len];

            match serde_json::from_slice::<ControlMessage>(packet_data) {
                Ok(message) => {
                    if let Err(e) = self.discovery_tx.send((message, remote_addr)).await {
                        eprintln!("Failed to send discovery to UI, receiver dropped: {}", e);
                        break;
                    }
                }
                Err(e) => {
                    eprintln!(
                        "Error parsing UDP packet: {}",
                        NetworkError::Serialization(e)
                    );
                    continue;
                }
            }
        }

        Ok(())
    }
}

pub struct DiscoveryBroadcaster {
    socket: UdpSocket,
    shutdown_rx: oneshot::Receiver<()>,
}

pub struct DiscoveryBroadcasterHandles {
    pub broadcaster: DiscoveryBroadcaster,
    pub shutdown_tx: oneshot::Sender<()>,
}

impl DiscoveryBroadcaster {
    #[expect(clippy::new_ret_no_self, reason = "returns a handles bundle by design")]
    pub async fn new() -> Result<DiscoveryBroadcasterHandles, GemaCastError> {
        let addr = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0);

        let socket = socket2::Socket::new(
            socket2::Domain::IPV4,
            socket2::Type::DGRAM,
            Some(socket2::Protocol::UDP),
        )
        .map_err(|e| NetworkError::BindFailed {
            addr: addr.to_string(),
            source: e,
        })?;

        socket
            .bind(&addr.into())
            .map_err(|e| NetworkError::BindFailed {
                addr: addr.to_string(),
                source: e,
            })?;

        socket
            .set_broadcast(true)
            .map_err(NetworkError::EnableBroadcastFailed)?;
        socket.set_multicast_ttl_v4(255).ok();

        socket.set_nonblocking(true).ok();
        let socket = UdpSocket::from_std(socket.into()).map_err(|e| NetworkError::BindFailed {
            addr: addr.to_string(),
            source: e,
        })?;

        let (shutdown_tx, shutdown_rx) = oneshot::channel();

        Ok(DiscoveryBroadcasterHandles {
            broadcaster: DiscoveryBroadcaster {
                socket,
                shutdown_rx,
            },
            shutdown_tx,
        })
    }

    pub async fn broadcast_presence<F, T>(
        mut self,
        mut payload_factory: F,
        mut target_ips: T,
    ) -> Result<(), NetworkError>
    where
        F: FnMut() -> ControlMessage + Send,
        T: FnMut() -> Vec<SocketAddrV4> + Send,
    {
        loop {
            let broadcast_addrs: Vec<SocketAddrV4> = super::get_broadcast_addrs()
                .into_iter()
                .map(|ip| SocketAddrV4::new(ip, DISCOVERY_PORT))
                .collect();
            let unicast_addrs = target_ips();
            let broadcast_addr_global =
                SocketAddrV4::new(Ipv4Addr::new(255, 255, 255, 255), DISCOVERY_PORT);
            let multicast_addr = SocketAddrV4::new(Ipv4Addr::new(224, 0, 0, 124), DISCOVERY_PORT);

            let mut payload = payload_factory();
            let json_bytes = serde_json::to_vec(&payload)?;

            // Rapid-fire 3 packets for reliability on congested bands like 2.4 GHz
            for _ in 0..3 {
                for addr in &broadcast_addrs {
                    let _ = self.socket.send_to(&json_bytes, *addr).await;
                }
                for addr in &unicast_addrs {
                    let _ = self.socket.send_to(&json_bytes, *addr).await;
                }
                let _ = self
                    .socket
                    .send_to(&json_bytes, broadcast_addr_global)
                    .await;
                let _ = self.socket.send_to(&json_bytes, multicast_addr).await;
                
                tokio::time::sleep(Duration::from_millis(25)).await;
            }

            tokio::select! {
                // Wait the remainder of the 1-second interval minus the 50ms from retries
                _ = sleep(Duration::from_millis(950)) => {}
                _ = &mut self.shutdown_rx => {
                    if let ControlMessage::Presence { ref mut is_offline, .. } = payload {
                        *is_offline = true;
                    }
                    if let Ok(offline_bytes) = serde_json::to_vec(&payload) {
                        for _ in 0..3 {
                            for addr in &broadcast_addrs {
                                let _ = self.socket.send_to(&offline_bytes, *addr).await;
                            }
                            for addr in &unicast_addrs {
                                let _ = self.socket.send_to(&offline_bytes, *addr).await;
                            }
                            let _ = self.socket.send_to(&offline_bytes, broadcast_addr_global).await;
                            let _ = self.socket.send_to(&offline_bytes, multicast_addr).await;
                        }
                    }
                    break;
                }
            }
        }

        Ok(())
    }
}

pub async fn send_control_message(
    target_ip: std::net::IpAddr,
    message: ControlMessage,
) -> Result<(), NetworkError> {
    let addr: SocketAddrV4 = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0);
    let socket = UdpSocket::bind(addr)
        .await
        .map_err(|e| NetworkError::BindFailed {
            addr: addr.to_string(),
            source: e,
        })?;

    let target_addr = std::net::SocketAddr::new(target_ip, DISCOVERY_PORT);
    let json_bytes = serde_json::to_vec(&message)?;

    // Send multiple times rapidly to ensure delivery
    let mut last_err = None;
    for _ in 0..3 {
        match socket.send_to(&json_bytes, target_addr).await {
            Ok(_) => {}
            Err(e) => last_err = Some(e),
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    if let Some(e) = last_err {
        return Err(NetworkError::SendFailed(e));
    }
    Ok(())
}
