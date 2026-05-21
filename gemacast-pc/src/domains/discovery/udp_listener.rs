use gemacast_core::network::PresenceListener;
use gemacast_core::types::ControlMessage;
use std::net::SocketAddr;
use tao::event_loop::EventLoopProxy;
use tokio::task::JoinSet;

use crate::events::DaemonEvent;

pub fn spawn_udp_listener(
    set: &mut JoinSet<()>,
    listener: PresenceListener,
    mut presence_message_rx: tokio::sync::mpsc::Receiver<(ControlMessage, SocketAddr)>,
    inbound_control_message_tx: tokio::sync::mpsc::Sender<(ControlMessage, SocketAddr)>,
    proxy_for_discovery: EventLoopProxy<DaemonEvent>,
) {
    set.spawn(async move {
        if let Err(e) = listener.run_receive_loop().await {
            let _ = proxy_for_discovery.send_event(DaemonEvent::FatalError(e.to_string()));
        }
    });

    let inbound_tx_for_discovery = inbound_control_message_tx;
    set.spawn(async move {
        while let Some(msg) = presence_message_rx.recv().await {
            let _ = inbound_tx_for_discovery.send(msg).await;
        }
    });
}
