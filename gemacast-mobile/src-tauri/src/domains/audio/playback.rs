use std::sync::atomic::AtomicBool;
use std::sync::{Arc, RwLock};
use tauri::{Emitter, Manager};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use gemacast_core::stream::receiver::AudioStreamReceiver;
use gemacast_core::types::JitterConfig;

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

    #[derive(serde::Serialize, Clone)]
    #[serde(rename_all = "camelCase")]
    struct AudioTelemetry {
        latency: f32,
        is_active: bool,
    }

    let (latency_tx, mut latency_rx) = tokio::sync::mpsc::channel::<(f32, f32)>(10);
    let handle_latency = app_handle.clone();
    tokio::spawn(async move {
        let mut last_emit = std::time::Instant::now();
        while let Some((latency, rms)) = latency_rx.recv().await {
            if last_emit.elapsed() >= std::time::Duration::from_millis(200) {
                last_emit = std::time::Instant::now();
                let _ = handle_latency.emit(
                    "audio-telemetry",
                    AudioTelemetry {
                        latency,
                        is_active: rms > 0.0001,
                    },
                );
            }
        }
    });

    (sender_ip_tx, latency_tx)
}

pub type SessionReceiverResult = Result<
    (
        Arc<AtomicBool>,
        Arc<AtomicBool>,
        Arc<RwLock<JitterConfig>>,
        oneshot::Sender<()>,
        JoinHandle<()>,
    ),
    String,
>;

pub fn spawn_session_receiver(
    jitter_config: JitterConfig,
    is_tcp: bool,
    exclusive_mode: bool,
    app_handle: tauri::AppHandle,
    target_ip: Option<std::net::IpAddr>,
    mode: gemacast_core::types::ConnectionMode,
    device_id: String,
) -> SessionReceiverResult {
    let config_ref = Arc::new(RwLock::new(jitter_config));
    let is_tcp_mode = Arc::new(AtomicBool::new(is_tcp));
    let is_playing = Arc::new(AtomicBool::new(true));
    let volume = Arc::new(std::sync::atomic::AtomicU32::new(f32::to_bits(1.0)));
    let (shutdown_tx, shutdown_rx) = oneshot::channel();

    let mut receiver = AudioStreamReceiver::new(
        config_ref.clone(),
        is_tcp_mode.clone(),
        is_playing.clone(),
        volume,
        exclusive_mode,
        shutdown_rx,
    )
    .map_err(|e| e.to_string())?;

    let (sender_ip_tx, latency_tx) = setup_event_forwarding(app_handle.clone());

    let task = tokio::spawn(async move {
        if let Err(e) = receiver.activate_playback_stream() {
            let _ = app_handle.emit("playback-error", e.to_string());
            return;
        }

        if let Err(e) = receiver
            .run_audio_receive_loop(
                Some(sender_ip_tx),
                Some(latency_tx),
                target_ip,
                mode,
                device_id,
            )
            .await
        {
            if matches!(
                e,
                gemacast_core::error::GemaCastError::Network(
                    gemacast_core::error::NetworkError::ConnectionLost
                )
            ) {
                let _ = app_handle.emit("force-disconnect", ());
            } else {
                let _ = app_handle.emit("playback-error", e.to_string());
            }
        }
    });

    Ok((is_playing, is_tcp_mode, config_ref, shutdown_tx, task))
}
