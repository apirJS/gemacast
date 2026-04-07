use std::sync::atomic::Ordering;
use tauri::{Emitter, Manager};

use crate::state::{lock, AppState};

/// Spawns a loopback UDP server that the Android Java `ForegroundService` uses
/// to send commands (`RESUME`, `STOP_STREAM`, `DISCONNECT`) into the Rust core.
///
/// The assigned port is written to `<app_cache>/.ipc_port` so the Java side can
/// read it at startup without hard-coding a port number.
pub fn spawn_service_command_listener(app_handle: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        let addr = std::net::SocketAddrV4::new(std::net::Ipv4Addr::new(127, 0, 0, 1), 0);
        let Ok(socket) = tokio::net::UdpSocket::bind(addr).await else {
            eprintln!("[service] Failed to bind IPC socket");
            return;
        };

        if let Ok(local_addr) = socket.local_addr() {
            if let Ok(cache_dir) = app_handle.path().app_cache_dir() {
                let _ = std::fs::create_dir_all(&cache_dir);
                let _ = std::fs::write(cache_dir.join(".ipc_port"), local_addr.port().to_string());
            }
        }

        let mut buf = vec![0u8; 1024];
        while let Ok((len, _)) = socket.recv_from(&mut buf).await {
            let Ok(command) = std::str::from_utf8(&buf[..len]) else {
                continue;
            };

            handle_service_command(command, &app_handle).await;
            let _ = app_handle.emit("service-command", command);
        }
    });
}

/// Dispatches a single service command string to the appropriate handler.
async fn handle_service_command(command: &str, app_handle: &tauri::AppHandle) {
    match command {
        "RESUME" => handle_resume(app_handle).await,
        "STOP_STREAM" | "DISCONNECT" => handle_stop(app_handle).await,
        _ => {}
    }
}

/// Resumes audio playback and re-sends a `Connect` message to the PC.
async fn handle_resume(app_handle: &tauri::AppHandle) {
    let Some(state) = app_handle.try_state::<AppState>() else {
        return;
    };

    if let Ok(guard) = lock(&state.is_playing) {
        if let Some(flag) = guard.as_ref() {
            flag.store(true, Ordering::Relaxed);
        }
    }

    let ip = state.connected_ip.lock().ok().and_then(|g| *g);
    let did = state.device_id.lock().ok().and_then(|g| g.clone());
    let dname = state.device_name.lock().ok().and_then(|g| g.clone());

    if let (Some(ip_addr), Some(device_id), Some(device_name)) = (ip, did, dname) {
        let _ = gemacast_core::network::send_control_message(
            ip_addr,
            gemacast_core::types::ControlMessage::Connect {
                device_id,
                device_name,
            },
        )
        .await;
    }
}

/// Stops the local stream and notifies the PC.
async fn handle_stop(app_handle: &tauri::AppHandle) {
    let Some(state) = app_handle.try_state::<AppState>() else {
        return;
    };

    let ip = state.connected_ip.lock().ok().and_then(|g| *g);
    let did = state.device_id.lock().ok().and_then(|g| g.clone());

    if let (Some(ip_addr), Some(device_id)) = (ip, did) {
        let _ = gemacast_core::network::send_control_message(
            ip_addr,
            gemacast_core::types::ControlMessage::Disconnect { device_id },
        )
        .await;
    }

    crate::commands::playback::set_streaming_flag(app_handle, false);
}
