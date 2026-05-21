use std::sync::atomic::Ordering;
use tauri::{Emitter, Manager};

use gemacast_core::control::types::ConnectReq;
use gemacast_core::control::HttpControlClient;

use crate::state::AppState;

pub fn spawn_service_command_listener(app_handle: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        let addr = std::net::SocketAddrV4::new(std::net::Ipv4Addr::new(127, 0, 0, 1), 0);
        let Ok(socket) = tokio::net::UdpSocket::bind(addr).await else {
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

async fn handle_service_command(command: &str, app_handle: &tauri::AppHandle) {
    match command {
        "RESUME" => handle_resume_command(app_handle).await,
        "STOP_STREAM" | "DISCONNECT" => handle_stop_command(app_handle).await,
        _ => {}
    }
}

async fn handle_resume_command(app_handle: &tauri::AppHandle) {
    let Some(state) = app_handle.try_state::<AppState>() else {
        return;
    };

    let session_guard = state.session.lock().await;
    
    if let Some(session) = session_guard.as_ref() {
        session.is_playing.store(true, Ordering::Relaxed);

        let client = HttpControlClient::new(session.ip);
        let _ = client
            .send_connect_request(ConnectReq {
                device_id: session.device_id.clone(),
                device_name: session.device_name.clone(),
                source: gemacast_core::types::AudioSource::default(),
                mode: gemacast_core::types::ConnectionMode::default(),
                jitter_config: gemacast_core::types::JitterConfig::default(),
            })
            .await;
    }
}

async fn handle_stop_command(app_handle: &tauri::AppHandle) {
    let Some(state) = app_handle.try_state::<AppState>() else {
        return;
    };

    if let Some(session) = state.session.lock().await.as_ref() {
        let client = HttpControlClient::new(session.ip);
        let _ = client.send_disconnect_request(session.device_id.clone()).await;
    }

    crate::domains::audio::playback::set_streaming_flag(app_handle, false);
}
