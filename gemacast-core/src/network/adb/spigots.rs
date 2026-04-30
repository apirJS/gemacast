use crate::network::Ports;
use crate::types::{ControlMessage, DeviceId, SenderId};
use std::sync::Arc;
use tokio::task::JoinSet;

use super::framer::TcpAudioFramer;

/// Trait abstracting the broadcaster state needed by the ADB discovery spigot.
///
/// Decouples the spigot from platform-specific state. Consumers implement
/// this for their platform and pass it in.
pub trait PresenceProvider: Send + Sync + 'static {
    fn is_broadcasting(&self) -> bool;
    fn sender_id(&self) -> SenderId;
    fn sender_name(&self) -> String;
}

/// Binds TCP on the ADB audio port and frames binary audio packets from
/// the engine broadcast channel using [`TcpAudioFramer`].
pub fn spawn_audio_spigot(
    set: &mut JoinSet<()>,
    tcp_broadcaster_tx: tokio::sync::broadcast::Sender<Arc<Vec<u8>>>,
    tcp_drop_tx_for_audio: tokio::sync::broadcast::Sender<()>,
) {
    set.spawn(async move {
        let listener = {
            let mut attempts = 0;
            loop {
                let addr = format!("127.0.0.1:{}", Ports::ADB_AUDIO_TCP);
                match tokio::net::TcpListener::bind(&addr).await {
                    Ok(l) => break l,
                    Err(e) => {
                        attempts += 1;
                        if attempts >= 10 {
                            eprintln!("Failed to bind ADB audio TCP listener after 10 attempts: {}", e);
                            std::process::exit(1);
                        }
                        
                        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                    }
                }
            }
        };

        loop {
            let (mut socket, _) = match listener.accept().await {
                Ok(s) => s,
                Err(_) => continue,
            };
            let _ = socket.set_nodelay(true);
            
            let mut rx = tcp_broadcaster_tx.subscribe();
            
            let mut drop_rx = tcp_drop_tx_for_audio.subscribe();

            tokio::spawn(async move {
                let mut framer = TcpAudioFramer::new();
                loop {
                    tokio::select! {
                        msg = rx.recv() => {
                            match msg {
                                Ok(packet) => {
                                    framer.clear();
                                    framer.append_packet(&packet);

                                    while let Ok(msg2) = rx.try_recv() {
                                        framer.append_packet(&msg2);
                                    }

                                    if framer.flush(&mut socket).await.is_err() {
                                        
                                        break;
                                    }
                                }
                                Err(tokio::sync::broadcast::error::RecvError::Lagged(_n)) => {
                                    
                                    // Drain stale packets to catch up rather than playing old audio
                                    while rx.try_recv().is_ok() {}
                                    continue;
                                }
                                Err(_) => break,
                            }
                        }
                        _ = drop_rx.recv() => {
                            break;
                        }
                    }
                }
            });
        }
    });
}

/// Binds TCP on the ADB discovery port and handles discovery handshakes /
/// keepalive loops with ADB clients.
pub fn spawn_discovery_spigot<P: PresenceProvider>(
    set: &mut JoinSet<()>,
    presence_provider: Arc<P>,
    combined_tx_for_tcp: tokio::sync::mpsc::Sender<(ControlMessage, std::net::SocketAddr)>,
    tcp_drop_tx_for_discovery: tokio::sync::broadcast::Sender<()>,
    adb_control_tx: tokio::sync::broadcast::Sender<ControlMessage>,
) {
    set.spawn(async move {
        let listener = {
            let mut attempts = 0;
            loop {
                let addr = format!("127.0.0.1:{}", Ports::ADB_DISCOVERY_TCP);
                match tokio::net::TcpListener::bind(&addr).await {
                    Ok(l) => break l,
                    Err(e) => {
                        attempts += 1;
                        if attempts >= 10 {
                            eprintln!("Failed to bind ADB discovery TCP listener after 10 attempts: {}", e);
                            std::process::exit(1);
                        }
                        
                        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                    }
                }
            }
        };

        loop {
            let (mut socket, _) = match listener.accept().await {
                Ok(s) => s,
                Err(_) => continue,
            };

            let pp = presence_provider.clone();
            let is_brdcst = pp.is_broadcasting();
            let sid = pp.sender_id();
            let sname = pp.sender_name();
            let sid_task = sid.clone();
            let sname_task = sname.clone();
            let is_offline = !is_brdcst;

            let presence = ControlMessage::Presence {
                sender_id: sid,
                sender_name: sname,
                is_offline,
                transport: None,
            };

            let mut json = match serde_json::to_string(&presence) {
                Ok(j) => j,
                Err(_) => continue,
            };
            json.push('\n');

            let combined_tx_clone = combined_tx_for_tcp.clone();
            let mut drop_rx = tcp_drop_tx_for_discovery.subscribe();
            let mut adb_control_rx = adb_control_tx.subscribe();
            let pp_clone = presence_provider.clone();

            tokio::spawn(async move {
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                let _ = socket.write_all(json.as_bytes()).await;

                let mut buf = vec![0u8; 2048];
                let mut accum = Vec::new();
                let mut keepalive_interval =
                    tokio::time::interval(tokio::time::Duration::from_millis(1500));

                let mut adb_device_id: Option<DeviceId> = None;

                loop {
                    tokio::select! {
                        _ = keepalive_interval.tick() => {
                            if let Some(ref adb_did) = adb_device_id {
                                let probe = ControlMessage::Probe {
                                    device_id: Some(adb_did.clone()),
                                };
                                let _ = combined_tx_clone
                                    .send((
                                        probe,
                                        std::net::SocketAddr::new(
                                            std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
                                            Ports::ADB_DISCOVERY_TCP,
                                        ),
                                    ))
                                    .await;
                            }

                            let is_brcst_now = pp_clone.is_broadcasting();
                            let presence_update = ControlMessage::Presence {
                                sender_id: sid_task.clone(),
                                sender_name: sname_task.clone(),
                                is_offline: !is_brcst_now,
                                transport: None,
                            };
                            if let Ok(mut json) = serde_json::to_string(&presence_update) {
                                json.push('\n');
                                let _ = socket.write_all(json.as_bytes()).await;
                            }
                        }
                        Ok(control_msg) = adb_control_rx.recv() => {
                            if let Ok(mut json) = serde_json::to_string(&control_msg) {
                                json.push('\n');
                                let _ = socket.write_all(json.as_bytes()).await;
                            }
                        }
                        result = socket.read(&mut buf) => {
                            match result {
                                Ok(0) => break,
                                Ok(n) => {
                                    accum.extend_from_slice(&buf[..n]);
                                    let mut start = 0;
                                    while let Some(pos) = accum[start..].iter().position(|&b| b == b'\n') {
                                        let chunk = &accum[start..start + pos];
                                        if let Ok(msg) = serde_json::from_slice::<ControlMessage>(chunk) {
                                            match &msg {
                                                ControlMessage::Connect { device_id, .. } => {
                                                    adb_device_id = Some(device_id.clone());
                                                }
                                                ControlMessage::Probe { device_id: Some(id), .. } => {
                                                    adb_device_id = Some(id.clone());
                                                }
                                                _ => {}
                                            }
                                            let peer = socket.peer_addr().unwrap_or(
                                                std::net::SocketAddr::new(
                                                    std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
                                                    Ports::ADB_DISCOVERY_TCP,
                                                ),
                                            );
                                            let _ = combined_tx_clone.send((msg, peer)).await;
                                        }
                                        start += pos + 1;
                                    }

                                    if start == 0 && n > 0 && accum.ends_with(b"}")
                                        && let Ok(msg) = serde_json::from_slice::<ControlMessage>(&accum) {
                                            match &msg {
                                                ControlMessage::Connect { device_id, .. } => {
                                                    adb_device_id = Some(device_id.clone());
                                                }
                                                ControlMessage::Probe { device_id: Some(id), .. } => {
                                                    adb_device_id = Some(id.clone());
                                                }
                                                _ => {}
                                            }
                                            let peer = socket.peer_addr().unwrap_or(
                                                std::net::SocketAddr::new(
                                                    std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
                                                    Ports::ADB_DISCOVERY_TCP,
                                                ),
                                            );
                                            let _ = combined_tx_clone.send((msg, peer)).await;
                                            accum.clear();
                                            start = accum.len();
                                        }

                                    accum.drain(..start);
                                }
                                Err(_) => break,
                            }
                        }
                        _ = drop_rx.recv() => break,
                    }
                }
            });
        }
    });
}
