use crate::error::{GemaCastError, NetworkError};
use crate::types::{BroadcastPayload, DiscoveredDevice};
use std::net::{Ipv4Addr, SocketAddrV4};
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, oneshot};
use tokio::time::sleep;

use super::{AUDIO_PORT, DISCOVERY_PORT, get_broadcast_addr};

pub struct DiscoveryListener {
    discovery_tx: mpsc::Sender<DiscoveredDevice>,
}

pub struct DiscoveryListenerHandles {
    pub listener: DiscoveryListener,
    pub discovery_rx: mpsc::Receiver<DiscoveredDevice>,
}

impl DiscoveryListener {
    pub fn new() -> DiscoveryListenerHandles {
        let (discovery_tx, discovery_rx) = mpsc::channel::<DiscoveredDevice>(8);

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

            match serde_json::from_slice::<BroadcastPayload>(packet_data) {
                Ok(payload) => {
                    let mut audio_addr = remote_addr;
                    audio_addr.set_port(AUDIO_PORT);

                    let device = DiscoveredDevice::from_broadcast(payload, audio_addr);
                    if let Err(e) = self.discovery_tx.send(device).await {
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

    pub async fn broadcast_device(
        mut self,
        mut payload: BroadcastPayload,
    ) -> Result<(), NetworkError> {
        let json_bytes = serde_json::to_vec(&payload)?;
        let broadcast_addr_subnet = SocketAddrV4::new(get_broadcast_addr(), DISCOVERY_PORT);
        let broadcast_addr_global =
            SocketAddrV4::new(Ipv4Addr::new(255, 255, 255, 255), DISCOVERY_PORT);
        let multicast_addr = SocketAddrV4::new(Ipv4Addr::new(224, 0, 0, 124), DISCOVERY_PORT);

        loop {
            let _ = self
                .socket
                .send_to(&json_bytes, broadcast_addr_subnet)
                .await;
            let _ = self
                .socket
                .send_to(&json_bytes, broadcast_addr_global)
                .await;
            let _ = self.socket.send_to(&json_bytes, multicast_addr).await;

            tokio::select! {
                _ = sleep(Duration::from_secs(1)) => {}
                _ = &mut self.shutdown_rx => {
                    payload.is_offline = true;
                    if let Ok(offline_bytes) = serde_json::to_vec(&payload) {
                        for _ in 0..3 {
                            let _ = self.socket.send_to(&offline_bytes, broadcast_addr_subnet).await;
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
