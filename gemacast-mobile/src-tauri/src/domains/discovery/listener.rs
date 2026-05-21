use std::time::Instant;
use tauri::Emitter;
use tokio::task::JoinHandle;

use crate::HEARTBEAT_CHECK_INTERVAL_SECS;
use crate::SENDER_HEARTBEAT_TIMEOUT_SECS;

use gemacast_core::types::{ConnectionMode, DeviceId};

use super::dispatch::DispatchContext;

pub fn spawn_discovery_listener(
    listener: gemacast_core::network::PresenceListener,
    mut presence_message_rx: tokio::sync::mpsc::Receiver<(
        gemacast_core::types::ControlMessage,
        std::net::SocketAddr,
    )>,
    app_handle: tauri::AppHandle,
    device_id: DeviceId,
    mode: ConnectionMode,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut set = tokio::task::JoinSet::new();
        let ctx = DispatchContext::new(app_handle.clone());

        let socket = listener.socket.clone();
        set.spawn(async move {
            if let Err(e) = listener.run_receive_loop().await {
                eprintln!("Discovery listener failed: {}", e);
                std::process::exit(1);
            }
        });

        let sender_heartbeat_tracker = ctx.sender_last_seen.clone();
        let app_handle_for_watchdog = app_handle.clone();
        set.spawn(async move {
            if mode == ConnectionMode::Adb {
                return;
            }
            let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(
                HEARTBEAT_CHECK_INTERVAL_SECS,
            ));
            loop {
                interval.tick().await;

                let stale_senders: Vec<DeviceId> = {
                    let map = sender_heartbeat_tracker.lock().unwrap();
                    let now = Instant::now();
                    map.iter()
                        .filter(|(_, ts)| {
                            now.duration_since(**ts).as_secs() >= SENDER_HEARTBEAT_TIMEOUT_SECS
                        })
                        .map(|(id, _)| id.clone())
                        .collect()
                };

                for sender_id in &stale_senders {
                    let _ = app_handle_for_watchdog.emit("sender-timeout", sender_id.0.clone());
                }

                if !stale_senders.is_empty() {
                    let mut map = sender_heartbeat_tracker.lock().unwrap();
                    for id in &stale_senders {
                        map.remove(id);
                    }
                }
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
            app_handle.clone(),
        ));

        while let Some((message, addr)) = presence_message_rx.recv().await {
            ctx.dispatch(message, addr, mode);
        }
    })
}
