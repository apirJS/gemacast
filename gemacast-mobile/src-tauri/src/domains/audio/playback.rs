use std::sync::atomic::{AtomicBool, AtomicU32};
use std::sync::{Arc, RwLock};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use gemacast_core::domain::types::JitterConfig;

use crate::traits::FrontendNotifier;

pub fn setup_event_forwarding(
    notifier: Arc<dyn FrontendNotifier>,
) -> (
    oneshot::Sender<String>,
    tokio::sync::mpsc::Sender<(f32, f32)>,
) {
    let (sender_ip_tx, sender_ip_rx) = oneshot::channel::<String>();
    let notifier_conn = notifier.clone();
    tokio::spawn(async move {
        if let Ok(ip) = sender_ip_rx.await {
            notifier_conn.emit_sender_connected(ip);
        }
    });

    let (latency_tx, mut latency_rx) = tokio::sync::mpsc::channel::<(f32, f32)>(10);
    tokio::spawn(async move {
        let mut last_emit = std::time::Instant::now();
        while let Some((latency, rms)) = latency_rx.recv().await {
            if last_emit.elapsed() >= std::time::Duration::from_millis(200) {
                last_emit = std::time::Instant::now();
                notifier.emit_audio_telemetry(latency, rms > 0.0001);
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
        Arc<AtomicU32>,
        oneshot::Sender<()>,
        JoinHandle<()>,
    ),
    String,
>;

pub fn spawn_session_receiver(
    jitter_config: JitterConfig,
    is_tcp: bool,
    exclusive_mode: bool,
    notifier: Arc<dyn FrontendNotifier>,
    target_ip: Option<std::net::IpAddr>,
    mode: gemacast_core::domain::types::ConnectionMode,
    device_id: String,
) -> SessionReceiverResult {
    let config_ref = Arc::new(RwLock::new(jitter_config));
    let is_tcp_mode = Arc::new(AtomicBool::new(is_tcp));
    let is_playing = Arc::new(AtomicBool::new(true));
    let volume = Arc::new(AtomicU32::new(f32::to_bits(1.0)));
    let (shutdown_tx, shutdown_rx) = oneshot::channel();

    let mut receiver = gemacast_core::stream::receiver::AudioStreamReceiver::new(
        config_ref.clone(),
        is_tcp_mode.clone(),
        is_playing.clone(),
        volume.clone(),
        exclusive_mode,
        shutdown_rx,
    )
    .map_err(|e| e.to_string())?;

    let (sender_ip_tx, latency_tx) = setup_event_forwarding(notifier.clone());

    let task = tokio::spawn(async move {
        if let Err(e) = receiver.activate_playback_stream() {
            notifier.emit_playback_error(e.to_string());
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
                gemacast_core::domain::error::GemaCastError::Network(
                    gemacast_core::domain::error::NetworkError::ConnectionLost
                )
            ) {
                notifier.emit_force_disconnect();
            } else {
                notifier.emit_playback_error(e.to_string());
            }
        }
    });

    Ok((
        is_playing,
        is_tcp_mode,
        config_ref,
        volume,
        shutdown_tx,
        task,
    ))
}
