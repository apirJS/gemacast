use super::playback::{set_streaming_flag, spawn_session_receiver};
use crate::state::{ActiveSession, AppState};
use gemacast_core::control::types::ConnectReq;
use gemacast_core::control::HttpControlClient;
use gemacast_core::types::{DeviceId, TransportType};
use tauri::{Emitter, State};

#[tauri::command]
pub fn notify_streaming_stopped(app_handle: tauri::AppHandle) -> Result<(), String> {
    set_streaming_flag(&app_handle, false);
    Ok(())
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn connect_to_sender(
    ip: String,
    device_id: DeviceId,
    device_name: String,
    mode: gemacast_core::types::ConnectionMode,
    exclusive_mode: bool,
    jitter_config: gemacast_core::types::JitterConfig,
    _transport: Option<TransportType>,
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    let ip_addr = ip.parse::<std::net::IpAddr>().map_err(|e| e.to_string())?;
    let client = HttpControlClient::new(ip_addr);

    client
        .send_connect_request(ConnectReq {
            device_id: device_id.clone(),
            device_name: device_name.clone(),
            source: gemacast_core::types::AudioSource::default(),
            mode,
            jitter_config: jitter_config.clone(),
        })
        .await
        .map_err(|e| e.to_string())?;

    let is_tcp = mode == gemacast_core::types::ConnectionMode::Adb;

    let mut session_guard = state.session.lock().await;

    if let Some(existing) = session_guard.take() {
        let _ = existing.shutdown_tx.send(());
        existing.playback_task.abort();
    }

    let (is_playing, _is_tcp_mode, jitter_config_ref, shutdown_tx, playback_task) =
        spawn_session_receiver(
            jitter_config,
            is_tcp,
            exclusive_mode,
            app_handle.clone(),
            Some(ip_addr),
            mode,
        )?;

    *session_guard = Some(ActiveSession {
        ip: ip_addr,
        device_id,
        device_name,
        exclusive_mode,
        mode,
        is_playing,
        jitter_config: jitter_config_ref,
        shutdown_tx,
        playback_task,
    });

    set_streaming_flag(&app_handle, true);
    sync_android_service(true, exclusive_mode);

    Ok(())
}

#[tauri::command]
pub async fn disconnect_from_sender(
    ip: String,
    device_id: DeviceId,
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    let ip_addr = ip.parse::<std::net::IpAddr>().map_err(|e| e.to_string())?;
    let client = HttpControlClient::new(ip_addr);
    let _ = client.send_disconnect_request(device_id).await;

    if let Some(session) = state.session.lock().await.take() {
        let _ = session.shutdown_tx.send(());
        session.playback_task.abort();
    }

    set_streaming_flag(&app_handle, false);
    sync_android_service(false, false);
    Ok(())
}

#[tauri::command]
pub async fn stop_audio_playback(
    ip: Option<String>,
    device_id: Option<DeviceId>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    if let (Some(ip_str), Some(did)) = (ip, device_id) {
        if let Ok(ip_addr) = ip_str.parse::<std::net::IpAddr>() {
            let client = HttpControlClient::new(ip_addr);
            let _ = client.send_disconnect_request(did).await;
        }
    }

    if let Some(session) = state.session.lock().await.take() {
        let _ = session.shutdown_tx.send(());
        session.playback_task.abort();
    }

    sync_android_service(false, false);
    Ok(())
}

#[tauri::command]
pub async fn kill_playback(
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    if let Some(session) = state.session.lock().await.take() {
        let _ = session.shutdown_tx.send(());
        session.playback_task.abort();
    }

    set_streaming_flag(&app_handle, false);
    sync_android_service(false, false);
    Ok(())
}

#[tauri::command]
pub async fn start_audio_playback(
    ip: Option<String>,
    device_id: Option<DeviceId>,
    device_name: Option<String>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let mut exclusive_mode = false;

    let mut active_mode = gemacast_core::types::ConnectionMode::default();
    let mut active_jitter = gemacast_core::types::JitterConfig::default();

    if let Some(session) = state.session.lock().await.as_ref() {
        session
            .is_playing
            .store(true, std::sync::atomic::Ordering::Relaxed);
        exclusive_mode = session.exclusive_mode;
        active_mode = session.mode;
        if let Ok(guard) = session.jitter_config.read() {
            active_jitter = guard.clone();
        }
    }

    if let (Some(ip_str), Some(did), Some(dname)) = (ip, device_id, device_name) {
        if let Ok(ip_addr) = ip_str.parse::<std::net::IpAddr>() {
            let client = HttpControlClient::new(ip_addr);
            let _ = client
                .send_connect_request(ConnectReq {
                    device_id: did,
                    device_name: dname,
                    source: gemacast_core::types::AudioSource::default(),
                    mode: active_mode,
                    jitter_config: active_jitter,
                })
                .await;
        }
    }

    sync_android_service(true, exclusive_mode);
    Ok(())
}

#[tauri::command]
pub async fn update_jitter_config(
    jitter_config: gemacast_core::types::JitterConfig,
    state: State<'_, AppState>,
) -> Result<(), String> {
    if let Some(session) = state.session.lock().await.as_ref() {
        if let Ok(mut guard) = session.jitter_config.write() {
            *guard = jitter_config;
        }
    }
    Ok(())
}

#[tauri::command]
pub async fn get_audio_sources(
    ip: String,
) -> Result<
    (
        Vec<gemacast_core::types::AudioSource>,
        gemacast_core::types::SenderCapabilities,
    ),
    String,
> {
    let ip_addr = ip.parse::<std::net::IpAddr>().map_err(|e| e.to_string())?;
    let client = HttpControlClient::new(ip_addr);
    client
        .request_audio_sources()
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn probe_sender(
    ip: String,
    device_id: DeviceId,
) -> Result<gemacast_core::control::types::PresenceResponse, String> {
    let ip_addr = ip.parse::<std::net::IpAddr>().map_err(|e| e.to_string())?;
    let client = HttpControlClient::new(ip_addr);
    client
        .send_probe(Some(device_id))
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn change_audio_source(
    ip: String,
    device_id: DeviceId,
    source: gemacast_core::types::AudioSource,
) -> Result<(), String> {
    let ip_addr = ip.parse::<std::net::IpAddr>().map_err(|e| e.to_string())?;
    let client = HttpControlClient::new(ip_addr);
    client
        .send_change_source_request(device_id, source)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_process_list(
    ip: String,
) -> Result<Vec<gemacast_core::types::ProcessInfo>, String> {
    let ip_addr = ip.parse::<std::net::IpAddr>().map_err(|e| e.to_string())?;
    let client = HttpControlClient::new(ip_addr);
    client
        .request_process_list()
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn establish_websocket(
    sender_ip: String,
    device_id: String,
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    let ip_addr = sender_ip
        .parse::<std::net::IpAddr>()
        .map_err(|e| e.to_string())?;

    let ws_client = gemacast_core::control::WsControlClient::new(ip_addr, &device_id)
        .await
        .map_err(|e| format!("Failed to establish WebSocket: {}", e))?;

    let ws_client_arc = std::sync::Arc::new(tokio::sync::Mutex::new(ws_client));

    let mut ws_guard = state.ws_client.lock().await;
    *ws_guard = Some(ws_client_arc.clone());
    drop(ws_guard);

    let app_handle_clone = app_handle.clone();
    tokio::spawn(async move {
        let event_result = {
            let client = ws_client_arc.lock().await;
            client.recv_event().await
        };

        match event_result {
            Ok(gemacast_core::control::types::WsEvent::Disconnect) => {
                let _ = app_handle_clone.emit("ws-disconnect", ());
            }
            Err(e) => {
                eprintln!("WebSocket error: {}", e);
            }
        }
    });

    Ok(())
}

#[allow(unused_variables)]
fn sync_android_service(is_playing: bool, is_exclusive: bool) {
    #[cfg(target_os = "android")]
    {
        let action = if is_playing { "START" } else { "STOP_STREAM" };
        let _ = std::process::Command::new("am")
            .args([
                "startservice",
                "-a",
                action,
                "--ez",
                "EXCLUSIVE_MODE",
                if is_exclusive { "true" } else { "false" },
                "com.apir.gemacast/.GemaCastService",
            ])
            .spawn();
    }
}
