use gemacast_core::network::Ports;
use gemacast_core::types::{ConnectionMode, DeviceId};
use tauri::Emitter;

use super::dispatch::DispatchContext;

pub async fn run_adb_session(
    ctx: DispatchContext,
    device_id: DeviceId,
    mode: ConnectionMode,
    app_handle: tauri::AppHandle,
) {
    if mode != ConnectionMode::Adb {
        return;
    }

    let mut was_connected = false;
    let mut retry_delay = 500u64;
    let adb_addr = format!("127.0.0.1:{}", Ports::ADB_DISCOVERY_TCP);

    loop {
        match tokio::net::TcpStream::connect(&adb_addr).await {
            Ok(mut stream) => {
                was_connected = true;
                retry_delay = 500;

                if let Ok(ident_bytes) =
                    serde_json::to_vec(&gemacast_core::types::ControlMessage::Probe {
                        device_id: Some(device_id.clone()),
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
                            >(line_buf.trim_end())
                            {
                                if let gemacast_core::types::ControlMessage::Presence { .. } = &msg
                                {
                                    last_presence = Some(msg.clone());
                                }
                                let loopback = std::net::SocketAddr::new(
                                    std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
                                    Ports::ADB_DISCOVERY_TCP,
                                );
                                ctx.dispatch(msg, loopback, mode);
                            }
                        }
                        Ok(_) => break,
                        Err(_) => break,
                    }
                }

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
                    ctx.dispatch(last_msg, loopback, mode);
                }

                let _ = app_handle.emit("force-disconnect", ());
            }
            Err(_) => {
                if was_connected {
                    let _ = app_handle.emit("force-disconnect", ());
                    was_connected = false;
                }
                tokio::time::sleep(tokio::time::Duration::from_millis(retry_delay)).await;
                if retry_delay < 5000 {
                    retry_delay += 500;
                }
            }
        }
    }
}
