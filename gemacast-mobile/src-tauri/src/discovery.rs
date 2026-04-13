use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tauri::Emitter;
use tokio::task::JoinHandle;

use crate::HEARTBEAT_CHECK_INTERVAL_SECS;
use crate::SENDER_HEARTBEAT_TIMEOUT_SECS;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ConnectionMode {
    Wifi,
    Usb,
    Adb,
}

/// Spawns the full discovery pipeline as a single managed task.
///
/// Internally manages three child tasks:
/// - **listener** — receives `ControlMessage` packets from the network.
/// - **watchdog** — removes senders that have not sent a heartbeat recently.
/// - **probe**    — actively sends `Probe` messages across known subnets (USB tether support).
///
/// The returned [`JoinHandle`] can be aborted to tear down the entire pipeline.
pub fn spawn_discovery_listener(
    listener: gemacast_core::network::DiscoveryListener,
    mut discovery_rx: tokio::sync::mpsc::Receiver<(
        gemacast_core::types::ControlMessage,
        std::net::SocketAddr,
    )>,
    app_handle: tauri::AppHandle,
    device_id: String,
    mode: ConnectionMode,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let socket = listener.socket.clone();

        let listener_handle = tokio::spawn(async move {
            if let Err(e) = listener.start().await {
                eprintln!("[discovery] Listener crashed: {:?}", e);
            }
        });

        let last_seen: Arc<Mutex<HashMap<String, Instant>>> = Arc::new(Mutex::new(HashMap::new()));
        let active_usb_ips: Arc<Mutex<HashMap<String, Instant>>> =
            Arc::new(Mutex::new(HashMap::new()));

        let last_seen_watcher = last_seen.clone();
        let app_handle_watcher = app_handle.clone();
        let watchdog_handle = tokio::spawn(async move {
            let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(
                HEARTBEAT_CHECK_INTERVAL_SECS,
            ));
            loop {
                interval.tick().await;

                let stale: Vec<String> = {
                    let map = last_seen_watcher.lock().unwrap();
                    let now = Instant::now();
                    map.iter()
                        .filter(|(_, ts)| {
                            now.duration_since(**ts).as_secs() >= SENDER_HEARTBEAT_TIMEOUT_SECS
                        })
                        .map(|(id, _)| id.clone())
                        .collect()
                };

                for sender_id in &stale {
                    eprintln!("[discovery] Sender heartbeat timeout: {}", sender_id);
                    let _ = app_handle_watcher.emit("sender-timeout", sender_id.clone());
                }

                if !stale.is_empty() {
                    let mut map = last_seen_watcher.lock().unwrap();
                    for id in &stale {
                        map.remove(id);
                    }
                }
            }
        });

        let probe_handle = tokio::spawn(async move {
            let mut interval = tokio::time::interval(tokio::time::Duration::from_millis(1500));
            let payload = gemacast_core::types::ControlMessage::Probe {
                device_id: Some(device_id),
            };

            let Ok(json_bytes) = serde_json::to_vec(&payload) else {
                eprintln!("[discovery] Failed to serialize Probe message");
                return;
            };

            loop {
                interval.tick().await;

                let subnets = collect_local_subnets();
                for (b0, b1, b2) in subnets {
                    for host in 1..=254u8 {
                        let target = std::net::SocketAddrV4::new(
                            std::net::Ipv4Addr::new(b0, b1, b2, host),
                            gemacast_core::network::DISCOVERY_PORT,
                        );
                        let _ = socket.send_to(&json_bytes, target).await;
                    }
                }
            }
        });

        while let Some((message, addr)) = discovery_rx.recv().await {
            dispatch_message(message, addr, &last_seen, &active_usb_ips, &app_handle, mode);
        }

        listener_handle.abort();
        watchdog_handle.abort();
        probe_handle.abort();
    })
}

/// Returns a deduplicated list of `(a, b, c)` subnet prefixes derived from all
/// non-loopback IPv4 interfaces, with RNDIS defaults as a fallback.
fn collect_local_subnets() -> Vec<(u8, u8, u8)> {
    let mut subnets = Vec::new();
    for iface in netdev::get_interfaces() {
        for ip_net in iface.ipv4 {
            let ip = ip_net.addr();
            if !ip.is_loopback() {
                let o = ip.octets();
                subnets.push((o[0], o[1], o[2]));
            }
        }
    }
    if subnets.is_empty() {
        subnets.push((192, 168, 42));
        subnets.push((192, 168, 43));
    }
    subnets
}

/// Routes a single [`ControlMessage`] received from the network.
fn dispatch_message(
    message: gemacast_core::types::ControlMessage,
    addr: std::net::SocketAddr,
    last_seen: &Arc<Mutex<HashMap<String, Instant>>>,
    active_usb_ips: &Arc<Mutex<HashMap<String, Instant>>>,
    app_handle: &tauri::AppHandle,
    mode: ConnectionMode,
) {
    match message {
        gemacast_core::types::ControlMessage::Presence {
            sender_id,
            sender_name,
            is_offline,
            volume,
            is_muted,
        } => {
            handle_presence(
                sender_id,
                sender_name,
                is_offline,
                volume,
                is_muted,
                addr,
                last_seen,
                active_usb_ips,
                app_handle,
                mode,
            );
        }
        gemacast_core::types::ControlMessage::Disconnect { .. } => {
            let _ = app_handle.emit("force-disconnect", ());
        }
        _ => {}
    }
}

/// Processes a `Presence` message, updating heartbeat state and emitting
/// a `sender-discovered` event to the frontend.
#[allow(clippy::too_many_arguments)]
fn handle_presence(
    sender_id: String,
    sender_name: String,
    is_offline: bool,
    volume: Option<f32>,
    is_muted: Option<bool>,
    addr: std::net::SocketAddr,
    last_seen: &Arc<Mutex<HashMap<String, Instant>>>,
    active_usb_ips: &Arc<Mutex<HashMap<String, Instant>>>,
    app_handle: &tauri::AppHandle,
    mode: ConnectionMode,
) {
    if is_offline {
        last_seen.lock().unwrap().remove(&sender_id);
        active_usb_ips.lock().unwrap().remove(&sender_id);
    } else {
        last_seen
            .lock()
            .unwrap()
            .insert(sender_id.clone(), Instant::now());
    }

    let mut audio_addr = addr;
    audio_addr.set_port(gemacast_core::network::AUDIO_PORT);

    let is_usb = gemacast_core::network::is_usb_tether_ip(&audio_addr.ip());

    match mode {
        ConnectionMode::Wifi => {
            if is_usb { return; }
        }
        ConnectionMode::Usb => {
            if !is_usb { return; }
        }
        ConnectionMode::Adb => {
            if !audio_addr.ip().is_loopback() { return; }
        }
    }

    if !is_offline {
        if is_usb {
            active_usb_ips
                .lock()
                .unwrap()
                .insert(sender_id.clone(), Instant::now());
        } else {
            let has_recent_usb = active_usb_ips
                .lock()
                .unwrap()
                .get(&sender_id)
                .is_some_and(|ts| {
                    Instant::now().duration_since(*ts).as_secs() < SENDER_HEARTBEAT_TIMEOUT_SECS
                });
            if has_recent_usb {
                return;
            }
        }
    }

    let device = gemacast_core::types::DiscoveredDevice::from_presence(
        sender_id,
        sender_name,
        is_offline,
        audio_addr,
        volume,
        is_muted,
    );
    let _ = app_handle.emit("sender-discovered", device);
}
