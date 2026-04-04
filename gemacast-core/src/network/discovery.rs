use crate::error::{GemaCastError, NetworkError};
use crate::types::ControlMessage;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, oneshot};
use tokio::time::sleep;

use super::DISCOVERY_PORT;

pub struct DiscoveryListener {
    discovery_tx: mpsc::Sender<(ControlMessage, std::net::SocketAddr)>,
}

pub struct DiscoveryListenerHandles {
    pub listener: DiscoveryListener,
    pub discovery_rx: mpsc::Receiver<(ControlMessage, std::net::SocketAddr)>,
}

impl DiscoveryListener {
    pub fn new() -> DiscoveryListenerHandles {
        let (discovery_tx, discovery_rx) = mpsc::channel::<(ControlMessage, std::net::SocketAddr)>(8);

        DiscoveryListenerHandles {
            listener: DiscoveryListener { discovery_tx },
            discovery_rx,
        }
    }

    pub async fn start(&self) -> Result<(), GemaCastError> {
        let addr = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, DISCOVERY_PORT);
        let discovery_socket =
            UdpSocket::bind(addr)
                .await
                .map_err(|e| NetworkError::BindFailed {
                    addr: addr.to_string(),
                    source: e,
                })?;
        let multicast_ip = Ipv4Addr::new(224, 0, 0, 124);
        let _ = discovery_socket.join_multicast_v4(multicast_ip, Ipv4Addr::UNSPECIFIED);
        let mut buff = vec![0u8; 2048];

        loop {
            let (len, remote_addr) = match discovery_socket.recv_from(&mut buff).await {
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
    pub async fn new() -> Result<DiscoveryBroadcasterHandles, GemaCastError> {
        let addr = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0);
        let socket = UdpSocket::bind(addr)
            .await
            .map_err(|e| NetworkError::BindFailed {
                addr: addr.to_string(),
                source: e,
            })?;

        socket
            .set_broadcast(true)
            .map_err(|e| NetworkError::EnableBroadcastFailed(e))?;

        let (shutdown_tx, shutdown_rx) = oneshot::channel();

        Ok(DiscoveryBroadcasterHandles {
            broadcaster: DiscoveryBroadcaster {
                socket,
                shutdown_rx,
            },
            shutdown_tx,
        })
    }

    pub async fn broadcast_presence(
        mut self,
        mut payload: ControlMessage,
    ) -> Result<(), NetworkError> {
        let broadcast_addrs: Vec<SocketAddrV4> = super::get_broadcast_addrs()
            .into_iter()
            .map(|ip| SocketAddrV4::new(ip, DISCOVERY_PORT))
            .collect();
        let broadcast_addr_global =
            SocketAddrV4::new(Ipv4Addr::new(255, 255, 255, 255), DISCOVERY_PORT);
        let multicast_addr = SocketAddrV4::new(Ipv4Addr::new(224, 0, 0, 124), DISCOVERY_PORT);

        loop {
            let json_bytes = serde_json::to_vec(&payload)?;
            for addr in &broadcast_addrs {
                let _ = self.socket.send_to(&json_bytes, *addr).await;
            }
            let _ = self
                .socket
                .send_to(&json_bytes, broadcast_addr_global)
                .await;
            let _ = self.socket.send_to(&json_bytes, multicast_addr).await;

            tokio::select! {
                _ = sleep(Duration::from_secs(1)) => {}
                _ = &mut self.shutdown_rx => {
                    if let ControlMessage::Presence { ref mut is_offline, .. } = payload {
                        *is_offline = true;
                    }
                    if let Ok(offline_bytes) = serde_json::to_vec(&payload) {
                        for _ in 0..3 {
                            for addr in &broadcast_addrs {
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
    let addr = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0);
    let socket = UdpSocket::bind(addr)
        .await
        .map_err(|e| NetworkError::BindFailed {
            addr: addr.to_string(),
            source: e,
        })?;

    let target_addr = std::net::SocketAddr::new(target_ip, DISCOVERY_PORT);
    let json_bytes = serde_json::to_vec(&message)?;
    socket
        .send_to(&json_bytes, target_addr)
        .await
        .map_err(NetworkError::SendFailed)?;
    Ok(())
}
