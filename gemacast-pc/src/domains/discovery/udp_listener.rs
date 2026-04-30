use gemacast_core::network::DiscoveryListener;
use gemacast_core::types::ControlMessage;
use std::net::SocketAddr;
use tao::event_loop::EventLoopProxy;
use tokio::task::JoinSet;

use crate::events::DaemonEvent;

pub fn spawn_udp_listener(
    set: &mut JoinSet<()>,
    listener: DiscoveryListener,
    mut discovery_rx: tokio::sync::mpsc::Receiver<(ControlMessage, SocketAddr)>,
    combined_tx_for_udp: tokio::sync::mpsc::Sender<(ControlMessage, SocketAddr)>,
    proxy_for_discovery: EventLoopProxy<DaemonEvent>,
) {
    set.spawn(async move {
        if let Err(e) = listener.start().await {
            let _ = proxy_for_discovery.send_event(DaemonEvent::FatalError(e.to_string()));
        }
    });

    set.spawn(async move {
        while let Some(msg) = discovery_rx.recv().await {
            let _ = combined_tx_for_udp.send(msg).await;
        }
    });
}
