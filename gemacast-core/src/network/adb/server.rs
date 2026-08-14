use crate::control::messages::ControlMessage;
use crate::control::{SessionAuthorizer, SessionGeneration};
use crate::domain::types::DeviceId;
use crate::network::Ports;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::task::JoinSet;

use super::framer::TcpAudioFramer;

#[derive(Debug, PartialEq, Eq)]
struct AdbAudioHandshake {
    device_id: DeviceId,
    session_token: String,
    generation: SessionGeneration,
}

fn parse_adb_audio_handshake(bytes: &[u8]) -> Result<AdbAudioHandshake, &'static str> {
    use crate::stream::receiver::transport::{
        ADB_HANDSHAKE_VERSION, MAX_DEVICE_ID_LENGTH, MAX_SESSION_TOKEN_LENGTH,
    };

    let mut cursor = 0usize;
    let take = |cursor: &mut usize, len: usize| -> Option<&[u8]> {
        let end = cursor.checked_add(len)?;
        let value = bytes.get(*cursor..end)?;
        *cursor = end;
        Some(value)
    };

    if take(&mut cursor, 1) != Some(&[ADB_HANDSHAKE_VERSION][..]) {
        return Err("unsupported ADB audio handshake version");
    }
    let device_len = *take(&mut cursor, 1)
        .and_then(|value| value.first())
        .ok_or("missing ADB device ID length")? as usize;
    if device_len == 0 || device_len > MAX_DEVICE_ID_LENGTH {
        return Err("invalid ADB device ID length");
    }
    let device_id =
        std::str::from_utf8(take(&mut cursor, device_len).ok_or("truncated ADB device ID")?)
            .map_err(|_| "ADB device ID is not UTF-8")?;

    let token_len = *take(&mut cursor, 1)
        .and_then(|value| value.first())
        .ok_or("missing ADB session token length")? as usize;
    if token_len == 0 || token_len > MAX_SESSION_TOKEN_LENGTH {
        return Err("invalid ADB session token length");
    }
    let session_token =
        std::str::from_utf8(take(&mut cursor, token_len).ok_or("truncated ADB session token")?)
            .map_err(|_| "ADB session token is not UTF-8")?;

    let generation = u64::from_be_bytes(
        take(&mut cursor, 8)
            .ok_or("missing ADB session generation")?
            .try_into()
            .map_err(|_| "invalid ADB session generation")?,
    );
    if generation == 0 || cursor != bytes.len() {
        return Err("invalid ADB session generation or trailing data");
    }

    Ok(AdbAudioHandshake {
        device_id: DeviceId(device_id.to_string()),
        session_token: session_token.to_string(),
        generation: SessionGeneration(generation),
    })
}

async fn read_adb_audio_handshake(
    socket: &mut tokio::net::TcpStream,
) -> Result<AdbAudioHandshake, &'static str> {
    use crate::stream::receiver::transport::{MAX_DEVICE_ID_LENGTH, MAX_SESSION_TOKEN_LENGTH};
    use tokio::io::AsyncReadExt;

    let mut prefix = [0u8; 2];
    socket
        .read_exact(&mut prefix)
        .await
        .map_err(|_| "truncated ADB audio handshake")?;
    let device_len = prefix[1] as usize;
    if device_len == 0 || device_len > MAX_DEVICE_ID_LENGTH {
        return Err("invalid ADB device ID length");
    }

    let mut encoded = Vec::with_capacity(3 + device_len + MAX_SESSION_TOKEN_LENGTH + 8);
    encoded.extend_from_slice(&prefix);
    let mut device_and_token_len = vec![0u8; device_len + 1];
    socket
        .read_exact(&mut device_and_token_len)
        .await
        .map_err(|_| "truncated ADB audio handshake")?;
    let token_len = device_and_token_len[device_len] as usize;
    if token_len == 0 || token_len > MAX_SESSION_TOKEN_LENGTH {
        return Err("invalid ADB session token length");
    }
    encoded.extend_from_slice(&device_and_token_len);

    let mut token_and_generation = vec![0u8; token_len + 8];
    socket
        .read_exact(&mut token_and_generation)
        .await
        .map_err(|_| "truncated ADB audio handshake")?;
    encoded.extend_from_slice(&token_and_generation);
    parse_adb_audio_handshake(&encoded)
}

fn authorize_adb_audio_handshake(
    authorizer: &SessionAuthorizer,
    handshake: &AdbAudioHandshake,
) -> bool {
    authorizer
        .authenticate(&handshake.device_id, &handshake.session_token)
        .is_some_and(|session| session.generation == handshake.generation)
}

pub trait PresenceProvider: Send + Sync + 'static {
    fn is_broadcasting(&self) -> bool;
    fn sender_id(&self) -> DeviceId;
    fn sender_name(&self) -> String;
}

pub fn spawn_adb_audio_tcp_server(
    set: &mut JoinSet<()>,
    engine_command_tx: tokio::sync::mpsc::Sender<crate::stream::sender::engine::AudioStreamCommand>,
    tcp_drop_tx_for_audio: tokio::sync::broadcast::Sender<()>,
    error_tx: tokio::sync::mpsc::Sender<String>,
    authorizer: SessionAuthorizer,
) {
    set.spawn(async move {
        let socket_owners = Arc::new(std::sync::Mutex::new(HashMap::<
            DeviceId,
            (u64, tokio::sync::oneshot::Sender<()>),
        >::new()));
        let next_socket_id = Arc::new(AtomicU64::new(0));
        let listener = {
            let mut attempts = 0;
            loop {
                let addr_str = format!("127.0.0.1:{}", Ports::ADB_AUDIO_TCP);

                let bind_result = (|| -> Result<tokio::net::TcpListener, std::io::Error> {
                    let addr = addr_str.parse::<std::net::SocketAddr>().unwrap();
                    let socket = socket2::Socket::new(
                        socket2::Domain::IPV4,
                        socket2::Type::STREAM,
                        Some(socket2::Protocol::TCP),
                    )?;
                    socket.set_reuse_address(true).ok();
                    socket.bind(&addr.into())?;
                    socket.listen(128)?;
                    socket.set_nonblocking(true).ok();
                    tokio::net::TcpListener::from_std(socket.into())
                })();

                match bind_result {
                    Ok(l) => break l,
                    Err(e) => {
                        attempts += 1;
                        if attempts >= 10 {
                            let e_str = e.to_string();
                            let msg = if e_str.contains("Address already in use") || e_str.contains("10048") || e_str.contains("98") || e_str.contains("WSAEADDRINUSE") {
                                format!("ADB Audio Port ({}) is already in use by another application. Please check your Task Manager.", Ports::ADB_AUDIO_TCP)
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

            let engine_command_tx = engine_command_tx.clone();
            let mut drop_rx = tcp_drop_tx_for_audio.subscribe();
            let socket_owners = socket_owners.clone();
            let next_socket_id = next_socket_id.clone();
            let authorizer = authorizer.clone();

            tokio::spawn(async move {
                let handshake = match tokio::time::timeout(
                    tokio::time::Duration::from_secs(3),
                    read_adb_audio_handshake(&mut socket),
                )
                .await
                {
                    Ok(Ok(handshake)) => handshake,
                    Ok(Err(reason)) => {
                        tracing::warn!(%reason, "[ADB] Rejected malformed audio handshake");
                        return;
                    }
                    Err(_) => {
                        tracing::warn!("[ADB] Timed out waiting for audio handshake");
                        return;
                    }
                };
                if !authorize_adb_audio_handshake(&authorizer, &handshake) {
                    tracing::warn!(
                        device_id = %handshake.device_id,
                        generation = handshake.generation.0,
                        "[ADB] Rejected unauthorized audio socket"
                    );
                    return;
                }
                let typed_device_id = handshake.device_id;
                let device_id = typed_device_id.to_string();
                let socket_id = next_socket_id.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
                let (socket_replaced_tx, mut socket_replaced_rx) = tokio::sync::oneshot::channel();
                if let Ok(mut owners) = socket_owners.lock()
                    && let Some((_, old_socket_tx)) = owners.insert(
                        typed_device_id.clone(),
                        (socket_id, socket_replaced_tx),
                    )
                {
                    let _ = old_socket_tx.send(());
                }

                let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
                if engine_command_tx
                    .send(crate::stream::sender::engine::AudioStreamCommand::GetTcpBroadcaster {
                        device_id: typed_device_id.clone(),
                        reply: reply_tx,
                    })
                    .await
                    .is_err()
                {
                    tracing::error!("[ADB] Engine dropped before handshake completed for {}", device_id);
                    return;
                }

                let mut broadcaster = None;
                if let Ok(Some(b)) = reply_rx.await {
                    broadcaster = Some(b);
                } else {
                    for _ in 0..20 {
                        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
                        if engine_command_tx
                            .send(crate::stream::sender::engine::AudioStreamCommand::GetTcpBroadcaster {
                                device_id: typed_device_id.clone(),
                                reply: reply_tx,
                            })
                            .await
                            .is_err()
                        {
                            break;
                        }
                        if let Ok(Some(b)) = reply_rx.await {
                            broadcaster = Some(b);
                            break;
                        }
                    }
                }

                let lease = match broadcaster {
                    Some(b) => b,
                    _ => {
                        tracing::warn!("[ADB] No active source found for device={:?} after retries", device_id);
                        return;
                    }
                };

                let mut framer = TcpAudioFramer::new();
                let session_generation = lease.session_generation;
                let mut current_rx = lease.broadcaster.subscribe();
                drop(lease);

                loop {
                    tokio::select! {
                        // Forward audio packets from the current broadcast source
                        msg = current_rx.recv() => {
                            match msg {
                                Ok(packet) => {
                                    framer.clear();
                                    framer.append_packet(&packet);

                                    // Drain any queued packets
                                    while let Ok(msg2) = current_rx.try_recv() {
                                        framer.append_packet(&msg2);
                                    }

                                    if framer.flush(&mut socket).await.is_err() {
                                        break;
                                    }
                                }
                                Err(tokio::sync::broadcast::error::RecvError::Lagged(_n)) => {
                                    // Drain stale packets to catch up rather than playing old audio
                                    while current_rx.try_recv().is_ok() {}
                                    continue;
                                }
                                Err(_) => {
                                    // Broadcast channel closed — source was torn down or changed.
                                    // Try to fetch the new broadcaster from the engine.
                                    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
                                    if engine_command_tx
                                        .send(crate::stream::sender::engine::AudioStreamCommand::GetTcpBroadcaster {
                                            device_id: typed_device_id.clone(),
                                            reply: reply_tx,
                                        })
                                        .await
                                        .is_err()
                                    {
                                        break;
                                    }

                                    match reply_rx.await {
                                        Ok(Some(new_lease))
                                            if new_lease.session_generation == session_generation =>
                                        {
                                            current_rx = new_lease.broadcaster.subscribe();
                                            continue;
                                        }
                                        _ => break, // No active source found, actually shut down
                                    }
                                }
                            }
                        }
                        _ = drop_rx.recv() => {
                            break;
                        }
                        _ = &mut socket_replaced_rx => break,
                    }
                }

                let is_current = socket_owners
                    .lock()
                    .ok()
                    .and_then(|mut owners| {
                        (owners.get(&typed_device_id).map(|(id, _)| *id) == Some(socket_id))
                            .then(|| owners.remove(&typed_device_id))
                            .flatten()
                    })
                    .is_some();
                if is_current {
                    let _ = engine_command_tx
                        .send(
                            crate::stream::sender::engine::AudioStreamCommand::TransportClosed {
                                device_id: typed_device_id,
                                generation: session_generation,
                            },
                        )
                        .await;
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
                let addr_str = format!("127.0.0.1:{}", Ports::ADB_DISCOVERY_TCP);

                let bind_result = (|| -> Result<tokio::net::TcpListener, std::io::Error> {
                    let addr = addr_str.parse::<std::net::SocketAddr>().unwrap();
                    let socket = socket2::Socket::new(
                        socket2::Domain::IPV4,
                        socket2::Type::STREAM,
                        Some(socket2::Protocol::TCP),
                    )?;
                    socket.set_reuse_address(true).ok();
                    socket.bind(&addr.into())?;
                    socket.listen(128)?;
                    socket.set_nonblocking(true).ok();
                    tokio::net::TcpListener::from_std(socket.into())
                })();

                match bind_result {
                    Ok(l) => break l,
                    Err(e) => {
                        attempts += 1;
                        if attempts >= 10 {
                            let e_str = e.to_string();
                            let msg = if e_str.contains("Address already in use") || e_str.contains("10048") || e_str.contains("98") || e_str.contains("WSAEADDRINUSE") {
                                format!("ADB Discovery Port ({}) is already in use by another application. Please check your Task Manager.", Ports::ADB_DISCOVERY_TCP)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stream::receiver::transport::encode_adb_audio_handshake;

    #[test]
    fn authenticated_handshake_round_trips_and_requires_current_credentials() {
        let authorizer = SessionAuthorizer::default();
        let device_id = DeviceId("phone-1".into());
        let (token, generation) = authorizer.issue(device_id.clone()).unwrap();
        let encoded = encode_adb_audio_handshake(&device_id, &token, generation).unwrap();
        let handshake = parse_adb_audio_handshake(&encoded).unwrap();

        assert_eq!(handshake.device_id, device_id);
        assert!(authorize_adb_audio_handshake(&authorizer, &handshake));

        let mut stale = handshake;
        stale.generation = SessionGeneration(stale.generation.0 + 1);
        assert!(!authorize_adb_audio_handshake(&authorizer, &stale));
        stale.generation = generation;
        stale.session_token.push('x');
        assert!(!authorize_adb_audio_handshake(&authorizer, &stale));
    }

    #[test]
    fn malformed_handshake_is_rejected() {
        assert!(parse_adb_audio_handshake(&[1, 0]).is_err());
        assert!(
            parse_adb_audio_handshake(&[99, 1, b'x', 1, b'y', 0, 0, 0, 0, 0, 0, 0, 1]).is_err()
        );
    }
}
