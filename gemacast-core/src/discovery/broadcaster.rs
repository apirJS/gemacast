use crate::error::{GemaCastError, NetworkError};
use crate::network::Ports;
use crate::types::ControlMessage;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio::sync::oneshot;
use tokio::time::sleep;

pub struct PresenceBroadcaster {
    socket: UdpSocket,
    shutdown_rx: oneshot::Receiver<()>,
}

impl PresenceBroadcaster {
    pub async fn new(shutdown_rx: oneshot::Receiver<()>) -> Result<Self, GemaCastError> {
        tracing::info!("Initializing PresenceBroadcaster");
        let addr = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0);

        let socket = socket2::Socket::new(
            socket2::Domain::IPV4,
            socket2::Type::DGRAM,
            Some(socket2::Protocol::UDP),
        )
        .map_err(|e| NetworkError::SocketBindFailed {
            addr: addr.to_string(),
            source: e,
        })?;

        socket
            .bind(&addr.into())
            .map_err(|e| NetworkError::SocketBindFailed {
                addr: addr.to_string(),
                source: e,
            })?;

        socket
            .set_broadcast(true)
            .map_err(NetworkError::EnableBroadcastFailed)?;
        socket.set_multicast_ttl_v4(255).ok();

        socket.set_nonblocking(true).ok();
        let socket =
            UdpSocket::from_std(socket.into()).map_err(|e| NetworkError::SocketBindFailed {
                addr: addr.to_string(),
                source: e,
            })?;

        Ok(Self {
            socket,
            shutdown_rx,
        })
    }

    pub async fn run_broadcast_loop<F, T>(
        mut self,
        mut presence_payload_factory: F,
        mut known_receiver_addresses: T,
    ) -> Result<(), NetworkError>
    where
        F: FnMut() -> ControlMessage + Send,
        T: FnMut() -> Vec<SocketAddrV4> + Send,
    {
        tracing::info!("Starting UDP presence broadcast loop");
        loop {
            let broadcast_addrs: Vec<SocketAddrV4> = crate::network::get_broadcast_addrs()
                .into_iter()
                .map(|ip| SocketAddrV4::new(ip, Ports::DISCOVERY))
                .collect();
            let unicast_addrs = known_receiver_addresses();
            let broadcast_addr_global =
                SocketAddrV4::new(Ipv4Addr::new(255, 255, 255, 255), Ports::DISCOVERY);
            let multicast_addr = SocketAddrV4::new(Ipv4Addr::new(224, 0, 0, 124), Ports::DISCOVERY);

            let mut payload = presence_payload_factory();
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
                // Wait the remainder of the 1-second interval minus the ~75ms from retries
                _ = sleep(Duration::from_millis(950)) => {}
                _ = &mut self.shutdown_rx => {
                    tracing::info!("PresenceBroadcaster shutting down");
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
