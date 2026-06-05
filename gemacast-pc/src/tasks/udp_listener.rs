//! Receives presence and probe messages on the UDP discovery port.
//!
//! Spawns two tasks:
//! 1. The [`PresenceListener`] receive loop (forwards raw messages to a channel).
//! 2. A relay that copies messages into the shared inbound control channel
//!    (merging UDP and ADB-sourced control messages into one stream).

use std::net::SocketAddr;
use std::sync::Arc;

use gemacast_core::network::PresenceListener;
use gemacast_core::types::ControlMessage;
use tokio::sync::mpsc;
use tokio::task::JoinSet;

use crate::traits::TrayNotifier;

/// Spawn the UDP presence listener and a relay into the inbound control channel.
pub fn spawn_udp_listener(
    set: &mut JoinSet<()>,
    listener: PresenceListener,
    mut presence_rx: mpsc::Receiver<(ControlMessage, SocketAddr)>,
    inbound_control_tx: mpsc::Sender<(ControlMessage, SocketAddr)>,
    tray: Arc<dyn TrayNotifier>,
) {
    set.spawn(async move {
        if let Err(e) = listener.run_receive_loop().await {
            tracing::error!("UDP listener failed: {}", e);
            tray.notify_fatal_error(e.to_string());
        }
    });

    set.spawn(async move {
        while let Some(msg) = presence_rx.recv().await {
            let _ = inbound_control_tx.send(msg).await;
        }
    });
}
