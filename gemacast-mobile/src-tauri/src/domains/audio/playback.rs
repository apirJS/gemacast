use std::sync::atomic::Ordering;
use tauri::{Emitter, Manager};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use gemacast_core::stream::receiver::AudioReceiver;

use crate::state::{lock, AppState};

/// Writes or removes the `.streaming_active` flag file used by the Android
/// foreground service to know whether audio is currently streaming.
pub fn set_streaming_flag(app_handle: &tauri::AppHandle, active: bool) {
    if let Ok(cache_dir) = app_handle.path().app_cache_dir() {
        let flag_path = cache_dir.join(".streaming_active");
        if active {
            let _ = std::fs::create_dir_all(&cache_dir);
            let _ = std::fs::write(&flag_path, "1");
        } else {
            let _ = std::fs::remove_file(&flag_path);
        }
    }
}

/// Sets the `is_playing` atomic flag.
pub fn set_playing_flag(state: &AppState, playing: bool) -> Result<(), String> {
    if let Some(flag) = lock(&state.is_playing)?.as_ref() {
        flag.store(playing, Ordering::Relaxed);
    }
    Ok(())
}

/// Clears the `is_playing` flag (pauses audio without destroying the stream).
pub fn stop_playback_flag(state: &AppState) -> Result<(), String> {
    set_playing_flag(state, false)
}

/// Spawns async relay tasks that forward playback events to the Tauri frontend.
///
/// Returns:
/// - A oneshot sender that, when resolved, emits `sender-connected` with the
///   sender's IP.
/// - An mpsc sender for `(latency_ms, rms)` pairs that emit `latency-update`
///   and `audio-active` events.
pub fn setup_event_forwarding(
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
pub fn spawn_playback_task(
    mut receiver: AudioReceiver,
    app_handle: tauri::AppHandle,
    sender_ip_tx: oneshot::Sender<String>,
    latency_tx: tokio::sync::mpsc::Sender<(f32, f32)>,
    target_ip: Option<std::net::IpAddr>,
    mode: gemacast_core::types::ConnectionMode,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        if let Err(e) = receiver.start_audio_playback() {
            
            let _ = app_handle.emit("playback-error", e.to_string());
            return;
        }

        if let Err(e) = receiver
            .start_audio_listener(Some(sender_ip_tx), Some(latency_tx), target_ip, mode)
            .await
        {
            
            let _ = app_handle.emit("playback-error", e.to_string());
        }
    })
}
