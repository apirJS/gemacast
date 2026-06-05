use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinHandle;

use crate::HEARTBEAT_CHECK_INTERVAL_SECS;
use crate::SENDER_HEARTBEAT_TIMEOUT_SECS;
use crate::traits::FrontendNotifier;

use gemacast_core::types::{ConnectionMode, DeviceId};

use super::dispatch::DispatchContext;

pub fn spawn_discovery_listener(
    listener: gemacast_core::network::PresenceListener,
    mut presence_message_rx: tokio::sync::mpsc::Receiver<(
        gemacast_core::types::ControlMessage,
        std::net::SocketAddr,
    )>,
    notifier: Arc<dyn FrontendNotifier>,
    device_id: DeviceId,
    mode: ConnectionMode,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut set = tokio::task::JoinSet::new();
        let ctx = DispatchContext::new(notifier.clone());

        let socket = listener.socket.clone();
        set.spawn(async move {
            if let Err(e) = listener.run_receive_loop().await {
                tracing::error!("Discovery listener failed: {}", e);
                std::process::exit(1);
            }
        });

        // Heartbeat watchdog — delegates tick logic to heartbeat::evict_stale_senders
        let sender_heartbeat_tracker = ctx.sender_last_seen.clone();
        let notifier_for_watchdog = notifier.clone();
        set.spawn(async move {
            if mode == ConnectionMode::Adb {
                return;
            }
            let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(
                HEARTBEAT_CHECK_INTERVAL_SECS,
            ));
            loop {
                interval.tick().await;
                super::heartbeat::evict_stale_senders(
                    notifier_for_watchdog.as_ref(),
                    &sender_heartbeat_tracker,
                    Duration::from_secs(SENDER_HEARTBEAT_TIMEOUT_SECS),
                );
            }
        });

        set.spawn(super::probe::run_probe_loop(
            socket,
            device_id.clone(),
            mode,
        ));

        set.spawn(super::adb_session::run_adb_session(
            ctx.clone(),
            device_id,
            mode,
            notifier.clone(),
        ));

        while let Some((message, addr)) = presence_message_rx.recv().await {
            ctx.dispatch(message, addr, mode);
        }
    })
}
