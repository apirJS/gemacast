use tauri::State;

use gemacast_core::stream::receiver::AudioReceiver;
use gemacast_core::types::{DeviceId, TransportType};

use crate::state::{lock, AppState};

use super::playback::{
    set_playing_flag, set_streaming_flag, setup_event_forwarding, spawn_playback_task,
    stop_playback_flag,
};

/// Notifies the Rust core that the Android foreground service has stopped
/// streaming. Clears the flag so the service UI does not show incorrectly.
#[tauri::command]
pub fn notify_streaming_stopped(app_handle: tauri::AppHandle) -> Result<(), String> {
    set_streaming_flag(&app_handle, false);
    Ok(())
}

/// Connects to a PC sender:
/// 1. Sends a `Connect` control message.
/// 2. Initialises the [`AudioReceiver`] (only on the first call).
/// 3. Sets the `is_playing` atomic flag to `true`.
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

    use gemacast_core::control::handler::ControlHandler;
    let handler = gemacast_core::control::UdpControlHandler::new(ip_addr);
    handler.handle_connect(
        device_id.clone(),
        device_name.clone(),
        gemacast_core::types::AudioSource::default(),
        mode,
        jitter_config.clone(),
    )
    .await
    .map_err(|e| e.to_string())?;

    *lock(&state.connected_ip)? = Some(ip_addr);
    *lock(&state.device_id)? = Some(device_id);
    *lock(&state.device_name)? = Some(device_name);

    stop_playback_flag(&state)?;

    let is_initialized = lock(&state.playback_handle)?.is_some();

    if !is_initialized {
        let config_ref = std::sync::Arc::new(std::sync::RwLock::new(jitter_config.clone()));
        let is_tcp_mode = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(
            mode == gemacast_core::types::ConnectionMode::Adb,
        ));
        let is_playing = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        let volume = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(f32::to_bits(1.0)));
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();

        let receiver = AudioReceiver::new(
            config_ref.clone(),
            is_tcp_mode.clone(),
            is_playing.clone(),
            volume.clone(),
            exclusive_mode,
            shutdown_rx,
        )
        .map_err(|e| e.to_string())?;

        *lock(&state.shutdown_playback_tx)? = Some(shutdown_tx);
        *lock(&state.is_playing)? = Some(is_playing);
        *lock(&state.config_ref)? = Some(config_ref);
        *lock(&state.is_tcp_mode)? = Some(is_tcp_mode);
        *lock(&state.exclusive_mode)? = Some(exclusive_mode);

        let (sender_ip_tx, latency_tx) = setup_event_forwarding(app_handle.clone());
        let playback_task = spawn_playback_task(
            receiver,
            app_handle.clone(),
            sender_ip_tx,
            latency_tx,
            Some(ip_addr),
            mode,
        );
        *lock(&state.playback_handle)? = Some(playback_task);
    } else {
        if let Some(config_ref) = lock(&state.config_ref)?.as_ref() {
            if let Ok(mut guard) = config_ref.write() {
                *guard = jitter_config.clone();
            }
        }
        if let Some(tcp_flag) = lock(&state.is_tcp_mode)?.as_ref() {
            let is_tcp = mode == gemacast_core::types::ConnectionMode::Adb;
            tcp_flag.store(is_tcp, std::sync::atomic::Ordering::Relaxed);
        }
    }

    set_playing_flag(&state, true)?;
    set_streaming_flag(&app_handle, true);
    sync_android_service(true, exclusive_mode);

    Ok(())
}

/// Sends a `Disconnect` control message and pauses local playback.
#[tauri::command]
pub async fn disconnect_from_sender(
    ip: String,
    device_id: DeviceId,
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    let ip_addr = ip.parse::<std::net::IpAddr>().map_err(|e| e.to_string())?;

    use gemacast_core::control::handler::ControlHandler;
    let handler = gemacast_core::control::UdpControlHandler::new(ip_addr);
    let _ = handler.handle_disconnect(device_id).await;

    stop_playback_flag(&state)?;
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
            use gemacast_core::control::handler::ControlHandler;
            let handler = gemacast_core::control::UdpControlHandler::new(ip_addr);
            let _ = handler.handle_disconnect(did).await;
        }
    }
    stop_playback_flag(&state)?;
    sync_android_service(false, false);
    Ok(())
}

/// Forcefully terminates the playback task and clears all state.
#[tauri::command]
pub async fn kill_playback(
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    if let Some(tx) = lock(&state.shutdown_playback_tx)?.take() {
        let _ = tx.send(());
    }

    if let Some(handle) = lock(&state.playback_handle)?.take() {
        handle.abort();
    }

    *lock(&state.is_playing)? = None;
    *lock(&state.config_ref)? = None;
    *lock(&state.is_tcp_mode)? = None;
    *lock(&state.exclusive_mode)? = None;
    *lock(&state.connected_ip)? = None;

    set_streaming_flag(&app_handle, false);

    sync_android_service(false, false);
    Ok(())
}

/// Resumes audio by setting the `is_playing` flag to `true`.
///
/// Optionally re-sends a `Connect` message so the PC resumes unicasting audio.
#[tauri::command]
pub async fn start_audio_playback(
    ip: Option<String>,
    device_id: Option<DeviceId>,
    device_name: Option<String>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let mode_flag = lock(&state.exclusive_mode)?.unwrap_or(false);
    if let (Some(ip_str), Some(did), Some(dname)) = (ip, device_id, device_name) {
        if let Ok(ip_addr) = ip_str.parse::<std::net::IpAddr>() {
            use gemacast_core::control::handler::ControlHandler;
            let handler = gemacast_core::control::UdpControlHandler::new(ip_addr);
            let _ = handler.handle_connect(
                did,
                dname,
                gemacast_core::types::AudioSource::default(),
                gemacast_core::types::ConnectionMode::default(),
                gemacast_core::types::JitterConfig::default(),
            ).await;
        }
    }
    set_playing_flag(&state, true)?;
    sync_android_service(true, mode_flag);
    Ok(())
}

/// Dynamically updates the jitter buffer configuration in real-time.
#[tauri::command]
pub async fn update_jitter_config(
    jitter_config: gemacast_core::types::JitterConfig,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let _has_ref = lock(&state.config_ref)?.is_some();

    if let Some(config_ref) = lock(&state.config_ref)?.as_ref() {
        if let Ok(mut guard) = config_ref.write() {
            *guard = jitter_config;
        }
    }
    Ok(())
}

#[tauri::command]
pub async fn get_audio_sources(ip: String) -> Result<(Vec<gemacast_core::types::AudioSource>, gemacast_core::types::SenderCapabilities), String> {
    let ip_addr = ip.parse::<std::net::IpAddr>().map_err(|e| e.to_string())?;
    use gemacast_core::control::handler::ControlHandler;
    let handler = gemacast_core::control::UdpControlHandler::new(ip_addr);
    handler.get_sources().await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn change_audio_source(
    ip: String,
    device_id: DeviceId,
    source: gemacast_core::types::AudioSource,
) -> Result<(), String> {
    let ip_addr = ip.parse::<std::net::IpAddr>().map_err(|e| e.to_string())?;
    use gemacast_core::control::handler::ControlHandler;
    let handler = gemacast_core::control::UdpControlHandler::new(ip_addr);
    handler.handle_change_source(device_id, source).await.map_err(|e| e.to_string())
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
