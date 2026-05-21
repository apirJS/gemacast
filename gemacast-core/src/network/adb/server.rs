use crate::network::Ports;
use crate::types::{ControlMessage, DeviceId};
use std::sync::Arc;
use tokio::task::JoinSet;

use super::framer::TcpAudioFramer;

pub trait PresenceProvider: Send + Sync + 'static {
    fn is_broadcasting(&self) -> bool;
    fn sender_id(&self) -> DeviceId;
    fn sender_name(&self) -> String;
}

pub fn spawn_adb_audio_tcp_server(
    set: &mut JoinSet<()>,
    tcp_source_watch_rx: tokio::sync::watch::Receiver<
        Option<tokio::sync::broadcast::Sender<Arc<Vec<u8>>>>,
    >,
    tcp_drop_tx_for_audio: tokio::sync::broadcast::Sender<()>,
    error_tx: tokio::sync::mpsc::Sender<String>,
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
                            let e_str = e.to_string();
                            let msg = if e_str.contains("Address already in use") || e_str.contains("10048") || e_str.contains("98") || e_str.contains("WSAEADDRINUSE") {
                                "ADB Audio Port (4040) is already in use by another application. Please check your Task Manager.".to_string()
                            } else {
                                format!("Failed to bind ADB audio TCP listener: {}", e)
                            };
                            let _ = error_tx.send(msg).await;
                            return;
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

            // Get the current broadcast sender (if any)
            let initial_broadcaster = tcp_source_watch_rx.borrow().clone();
            let mut source_watch = tcp_source_watch_rx.clone();
            let mut drop_rx = tcp_drop_tx_for_audio.subscribe();

            tokio::spawn(async move {
                let mut framer = TcpAudioFramer::new();
                let mut current_rx: Option<tokio::sync::broadcast::Receiver<Arc<Vec<u8>>>> =
                    initial_broadcaster.map(|tx| tx.subscribe());

                loop {
                    tokio::select! {
                        // Forward audio packets from the current broadcast source
                        msg = async {
                            match current_rx.as_mut() {
                                Some(rx) => rx.recv().await,
                                None => std::future::pending().await,
                            }
                        } => {
                            match msg {
                                Ok(packet) => {
                                    framer.clear();
                                    framer.append_packet(&packet);

                                    // Drain any queued packets
                                    if let Some(rx) = current_rx.as_mut() {
                                        while let Ok(msg2) = rx.try_recv() {
                                            framer.append_packet(&msg2);
                                        }
                                    }

                                    if framer.flush(&mut socket).await.is_err() {
                                        break;
                                    }
                                }
                                Err(tokio::sync::broadcast::error::RecvError::Lagged(_n)) => {
                                    // Drain stale packets to catch up rather than playing old audio
                                    if let Some(rx) = current_rx.as_mut() {
                                        while rx.try_recv().is_ok() {}
                                    }
                                    continue;
                                }
                                Err(_) => {
                                    // Channel closed — wait for source watch to provide a new one
                                    current_rx = None;
                                    continue;
                                }
                            }
                        }
                        // Source changed — re-subscribe to the new broadcast channel
                        _ = source_watch.changed() => {
                            let new_broadcaster = source_watch.borrow().clone();
                            current_rx = new_broadcaster.map(|tx| tx.subscribe());
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


pub fn spawn_adb_discovery_tcp_server<P: PresenceProvider>(
    set: &mut JoinSet<()>,
    presence_provider: Arc<P>,
    combined_tx_for_tcp: tokio::sync::mpsc::Sender<(ControlMessage, std::net::SocketAddr)>,
    tcp_drop_tx_for_discovery: tokio::sync::broadcast::Sender<()>,
    adb_control_tx: tokio::sync::broadcast::Sender<ControlMessage>,
    error_tx: tokio::sync::mpsc::Sender<String>,
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
                            let e_str = e.to_string();
                            let msg = if e_str.contains("Address already in use") || e_str.contains("10048") || e_str.contains("98") || e_str.contains("WSAEADDRINUSE") {
                                "ADB Discovery Port (4041) is already in use by another application. Please check your Task Manager.".to_string()
                            } else {
                                format!("Failed to bind ADB discovery TCP listener: {}", e)
                            };
                            let _ = error_tx.send(msg).await;
                            return;
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
                device_id: sid,
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
                                device_id: sid_task.clone(),
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
                                            if let ControlMessage::Probe { device_id: Some(id), .. } = &msg {
                                                adb_device_id = Some(id.clone());
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
