use std::sync::atomic::Ordering;
use tauri::{Emitter, Manager, State};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use gemacast_core::network::AudioReceiver;

use crate::state::{lock, AppState};

/// Writes or removes the `.streaming_active` flag file used by the Android
/// foreground service to know whether audio is currently streaming.
pub fn set_streaming_flag(app_handle: &tauri::AppHandle, active: bool) {
    if let Ok(cache_dir) = app_handle.path().app_cache_dir() {
        let flag_path: std::path::PathBuf = cache_dir.join(".streaming_active");
        if active {
            let _ = std::fs::create_dir_all(&cache_dir);
            let _ = std::fs::write(&flag_path, "1");
        } else {
            let _ = std::fs::remove_file(&flag_path);
        }
    }
}

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
pub async fn connect_to_sender(
    ip: String,
    device_id: String,
    device_name: String,
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    let ip_addr = ip.parse::<std::net::IpAddr>().map_err(|e| e.to_string())?;

    gemacast_core::network::send_control_message(
        ip_addr,
        gemacast_core::types::ControlMessage::Connect {
            device_id: device_id.clone(),
            device_name: device_name.clone(),
        },
    )
    .await
    .map_err(|e| e.to_string())?;

    *lock(&state.connected_ip)? = Some(ip_addr);
    *lock(&state.device_id)? = Some(device_id);
    *lock(&state.device_name)? = Some(device_name);

    stop_playback_flag(&state)?;

    let is_initialized = lock(&state.playback_handle)?.is_some();

    if !is_initialized {
        let audio_handles = AudioReceiver::create().await.map_err(|e| e.to_string())?;
        *lock(&state.shutdown_playback_tx)? = Some(audio_handles.shutdown_tx);
        *lock(&state.is_playing)? = Some(audio_handles.is_playing);

        let (sender_ip_tx, latency_tx) = setup_event_forwarding(app_handle.clone());
        let playback_task = spawn_playback_task(
            audio_handles.receiver,
            app_handle.clone(),
            sender_ip_tx,
            latency_tx,
            Some(ip_addr),
        );
        *lock(&state.playback_handle)? = Some(playback_task);
    }

    set_playing_flag(&state, true)?;
    set_streaming_flag(&app_handle, true);

    Ok(())
}

/// Sends a `Disconnect` control message and pauses local playback.
#[tauri::command]
pub async fn disconnect_from_sender(
    ip: String,
    device_id: String,
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    let ip_addr = ip.parse::<std::net::IpAddr>().map_err(|e| e.to_string())?;

    let _ = gemacast_core::network::send_control_message(
        ip_addr,
        gemacast_core::types::ControlMessage::Disconnect { device_id },
    )
    .await;

    stop_playback_flag(&state)?;
    set_streaming_flag(&app_handle, false);
    Ok(())
}

/// Pauses audio by clearing the `is_playing` flag.
///
/// Optionally sends a `Disconnect` message if `ip` and `device_id` are given.
#[tauri::command]
pub async fn stop_audio_playback(
    ip: Option<String>,
    device_id: Option<String>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    if let (Some(ip_str), Some(did)) = (ip, device_id) {
        if let Ok(ip_addr) = ip_str.parse::<std::net::IpAddr>() {
            let _ = gemacast_core::network::send_control_message(
                ip_addr,
                gemacast_core::types::ControlMessage::Disconnect { device_id: did },
            )
            .await;
        }
    }
    stop_playback_flag(&state)
}

/// Resumes audio by setting the `is_playing` flag to `true`.
///
/// Optionally re-sends a `Connect` message so the PC resumes unicasting audio.
#[tauri::command]
pub async fn start_audio_playback(
    ip: Option<String>,
    device_id: Option<String>,
    device_name: Option<String>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    if let (Some(ip_str), Some(did), Some(dname)) = (ip, device_id, device_name) {
        if let Ok(ip_addr) = ip_str.parse::<std::net::IpAddr>() {
            let _ = gemacast_core::network::send_control_message(
                ip_addr,
                gemacast_core::types::ControlMessage::Connect {
                    device_id: did,
                    device_name: dname,
                },
            )
            .await;
        }
    }
    set_playing_flag(&state, true)
}

/// Sets the `is_playing` atomic flag.
fn set_playing_flag(state: &AppState, playing: bool) -> Result<(), String> {
    if let Some(flag) = lock(&state.is_playing)?.as_ref() {
        flag.store(playing, Ordering::Relaxed);
    }
    Ok(())
}

/// Clears the `is_playing` flag (pauses audio without destroying the stream).
fn stop_playback_flag(state: &AppState) -> Result<(), String> {
    set_playing_flag(state, false)
}

/// Spawns async relay tasks that forward playback events to the Tauri frontend.
///
/// Returns:
/// - A oneshot sender that, when resolved, emits `sender-connected` with the
///   sender's IP.
/// - An mpsc sender for `(latency_ms, rms)` pairs that emit `latency-update`
///   and `audio-active` events.
fn setup_event_forwarding(
    app_handle: tauri::AppHandle,
) -> (
    oneshot::Sender<String>,
    tokio::sync::mpsc::Sender<(f32, f32)>,
) {
    let (sender_ip_tx, sender_ip_rx) = oneshot::channel::<String>();
    let handle_conn = app_handle.clone();
    tokio::spawn(async move {
        if let Ok(ip) = sender_ip_rx.await {
            let _ = handle_conn.emit("sender-connected", ip);
        }
    });

    let (latency_tx, mut latency_rx) = tokio::sync::mpsc::channel::<(f32, f32)>(10);
    let handle_latency = app_handle.clone();
    tokio::spawn(async move {
        while let Some((latency, rms)) = latency_rx.recv().await {
            let _ = handle_latency.emit("latency-update", latency);
            let is_active = rms > 0.0001;
            let _ = handle_latency.emit("audio-active", is_active);
        }
    });

    (sender_ip_tx, latency_tx)
}

/// Spawns the audio playback task that drives the permanent hardware stream.
fn spawn_playback_task(
    mut receiver: AudioReceiver,
    app_handle: tauri::AppHandle,
    sender_ip_tx: oneshot::Sender<String>,
    latency_tx: tokio::sync::mpsc::Sender<(f32, f32)>,
    target_ip: Option<std::net::IpAddr>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        if let Err(e) = receiver.start_audio_playback() {
            eprintln!("[playback] Start failed: {:?}", e);
            let _ = app_handle.emit("playback-error", e.to_string());
            return;
        }

        if let Err(e) = receiver
            .start_audio_listener(Some(sender_ip_tx), Some(latency_tx), target_ip)
            .await
        {
            eprintln!("[playback] Audio listener crashed: {:?}", e);
            let _ = app_handle.emit("playback-error", e.to_string());
        }
    })
}
