use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tauri::Emitter;
use tokio::task::JoinHandle;

use crate::HEARTBEAT_CHECK_INTERVAL_SECS;
use crate::SENDER_HEARTBEAT_TIMEOUT_SECS;

use gemacast_core::network::Ports;
use gemacast_core::types::{ConnectionMode, DeviceId, SenderId, TransportType};

/// Spawns the full discovery pipeline as a single managed task.
pub fn spawn_discovery_listener(
    listener: gemacast_core::network::DiscoveryListener,
    mut discovery_rx: tokio::sync::mpsc::Receiver<(
        gemacast_core::types::ControlMessage,
        std::net::SocketAddr,
    )>,
    app_handle: tauri::AppHandle,
    device_id: DeviceId,
    mode: ConnectionMode,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut set = tokio::task::JoinSet::new();

        let socket = listener.socket.clone();

        set.spawn(async move {
            if let Err(e) = listener.start().await {
                eprintln!("Discovery listener failed: {}", e);
                std::process::exit(1);
            }
        });

        let last_seen: Arc<Mutex<HashMap<SenderId, Instant>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let active_usb_ips: Arc<Mutex<HashMap<SenderId, Instant>>> =
            Arc::new(Mutex::new(HashMap::new()));

        let last_seen_watcher = last_seen.clone();
        let app_handle_watcher = app_handle.clone();
        set.spawn(async move {
            if mode == ConnectionMode::Adb {
                return;
            }
            let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(
                HEARTBEAT_CHECK_INTERVAL_SECS,
            ));
            loop {
                interval.tick().await;

                let stale: Vec<SenderId> = {
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
                    
                    let _ = app_handle_watcher.emit("sender-timeout", sender_id.0.clone());
                }

                if !stale.is_empty() {
                    let mut map = last_seen_watcher.lock().unwrap();
                    for id in &stale {
                        map.remove(id);
                    }
                }
            }
        });

        let device_id_probe = device_id.clone();
        set.spawn(async move {
            if mode == ConnectionMode::Adb {
                return;
            }
            let mut interval = tokio::time::interval(tokio::time::Duration::from_millis(1000));
            let payload = gemacast_core::types::ControlMessage::Probe {
                device_id: Some(device_id_probe),
            };

            let Ok(json_bytes) = serde_json::to_vec(&payload) else {
                
                return;
            };

            loop {
                interval.tick().await;

                // Sweep twice back-to-back to double the hit probability on bad Wi-Fi
                for _ in 0..2 {
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
                    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
                }
            }
        });

        let last_seen_adb = last_seen.clone();
        let active_usb_ips_adb = active_usb_ips.clone();
        let app_handle_adb = app_handle.clone();
        let device_id_adb = device_id.clone();

        set.spawn(async move {
            if mode != ConnectionMode::Adb {
                return;
            }
            let mut was_connected = false;
            let mut retry_delay = 500;
            let adb_addr = format!("127.0.0.1:{}", Ports::ADB_DISCOVERY_TCP);

            loop {
                match tokio::net::TcpStream::connect(&adb_addr).await {
                    Ok(mut stream) => {
                        was_connected = true;
                        retry_delay = 500;

                        // Send initial Probe to inform PC of our identity on this persistent socket
                        if let Ok(ident_bytes) =
                            serde_json::to_vec(&gemacast_core::types::ControlMessage::Probe {
                                device_id: Some(device_id_adb.clone()),
                            })
                        {
                            use tokio::io::AsyncWriteExt;
                            let mut packet = ident_bytes;
                            packet.push(b'\n');
                            let _ = stream.write_all(&packet).await;
                        }

                        use tokio::io::AsyncBufReadExt;
                        let mut reader = tokio::io::BufReader::new(stream);
                        let mut line_buf = String::new();
                        let mut last_presence = None;

                        loop {
                            line_buf.clear();
                            match tokio::time::timeout(
                                tokio::time::Duration::from_millis(3500),
                                reader.read_line(&mut line_buf),
                            )
                            .await
                            {
                                Ok(Ok(n)) if n > 0 => {
                                    if let Ok(msg) = serde_json::from_str::<
                                        gemacast_core::types::ControlMessage,
                                    >(
                                        line_buf.trim_end()
                                    ) {
                                        if let gemacast_core::types::ControlMessage::Presence {
                                            ..
                                        } = &msg
                                        {
                                            last_presence = Some(msg.clone());
                                        }
                                        let loopback = std::net::SocketAddr::new(
                                            std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
                                            Ports::ADB_DISCOVERY_TCP,
                                        );
                                        dispatch_message(
                                            msg,
                                            loopback,
                                            &last_seen_adb,
                                            &active_usb_ips_adb,
                                            &app_handle_adb,
                                            mode,
                                        );
                                    }
                                }
                                Ok(_) => break,  // Ok(0) EOF or Ok(Err) socket error
                                Err(_) => break, // Timeout: no presence sent for 3.5s -> connection is physically dead
                            }
                        }

                        // Stream died — immediately notify UI that this specific ADB sender is offline
                        // bypassing the 5-second staleness ticker rule.
                        if let Some(mut last_msg) = last_presence.take() {
                            if let gemacast_core::types::ControlMessage::Presence {
                                ref mut is_offline,
                                ..
                            } = last_msg
                            {
                                *is_offline = true;
                            }
                            let loopback = std::net::SocketAddr::new(
                                std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
                                Ports::ADB_DISCOVERY_TCP,
                            );
                            dispatch_message(
                                last_msg,
                                loopback,
                                &last_seen_adb,
                                &active_usb_ips_adb,
                                &app_handle_adb,
                                mode,
                            );
                        }

                        // Disconnected — clear sender from UI
                        let _ = app_handle_adb.emit("force-disconnect", ());
                    }
                    Err(_) => {
                        // PC not ready or ADB tunnel not active yet
                        if was_connected {
                            let _ = app_handle_adb.emit("force-disconnect", ());
                            was_connected = false;
                        }
                        tokio::time::sleep(tokio::time::Duration::from_millis(retry_delay)).await;
                        if retry_delay < 5000 {
                            retry_delay += 500;
                        }
                    }
                }
            }
        });

        while let Some((message, addr)) = discovery_rx.recv().await {
            dispatch_message(
                message,
                addr,
                &last_seen,
                &active_usb_ips,
                &app_handle,
                mode,
            );
        }
    })
}

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

fn dispatch_message(
    message: gemacast_core::types::ControlMessage,
    addr: std::net::SocketAddr,
    last_seen: &Arc<Mutex<HashMap<SenderId, Instant>>>,
    active_usb_ips: &Arc<Mutex<HashMap<SenderId, Instant>>>,
    app_handle: &tauri::AppHandle,
    mode: ConnectionMode,
) {
    match message {
        gemacast_core::types::ControlMessage::Presence {
            sender_id,
            sender_name,
            is_offline,
            transport,
        } => {
            handle_presence(
                sender_id,
                sender_name,
                is_offline,
                transport,
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

#[allow(clippy::too_many_arguments)]
fn handle_presence(
    sender_id: SenderId,
    sender_name: String,
    is_offline: bool,
    transport: Option<TransportType>,
    addr: std::net::SocketAddr,
    last_seen: &Arc<Mutex<HashMap<SenderId, Instant>>>,
    active_usb_ips: &Arc<Mutex<HashMap<SenderId, Instant>>>,
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

    let mut is_usb = transport == Some(TransportType::Usb);
    
    if !is_usb {
        is_usb = gemacast_core::network::is_usb_tether_ip(&addr.ip());
    }

    match mode {
        ConnectionMode::Wifi => {
            if is_usb {
                return;
            }
        }
        ConnectionMode::Usb => {
            if !is_usb {
                return;
            }
        }
        ConnectionMode::Adb => {
            if !audio_addr.ip().is_loopback() {
                return;
            }
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
        transport,
    );
    let _ = app_handle.emit("sender-discovered", device);
}
