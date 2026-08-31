use std::sync::atomic::{AtomicBool, AtomicU32};
use std::sync::{Arc, RwLock};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use gemacast_core::domain::types::JitterConfig;

use crate::traits::FrontendNotifier;

/// Channels `setup_event_forwarding` hands back to the receive loop: the
/// streamer-IP oneshot, the audio-telemetry tuple stream `(buffer_ms, rms,
/// jitter_ms)`, and the wire-RTT stream.
type EventForwardingChannels = (
    oneshot::Sender<String>,
    tokio::sync::mpsc::Sender<(f32, f32, f32)>,
    tokio::sync::mpsc::Sender<f32>,
);

pub fn setup_event_forwarding(notifier: Arc<dyn FrontendNotifier>) -> EventForwardingChannels {
    let (streamer_ip_tx, streamer_ip_rx) = oneshot::channel::<String>();
    let notifier_conn = notifier.clone();
    tokio::spawn(async move {
        if let Ok(ip) = streamer_ip_rx.await {
            notifier_conn.emit_streamer_connected(ip);
        }
    });

    // Raw wire RTT from the UDP echo ping (player intercepts the reflected
    // ping and sends the round-trip here, ~every 500 ms). ADB/loopback has no
    // UDP echo, so this channel simply stays quiet there and the UI shows `--`.
    let (rtt_tx, mut rtt_rx) = tokio::sync::mpsc::channel::<f32>(10);
    let notifier_rtt = notifier.clone();
    tokio::spawn(async move {
        while let Some(rtt_ms) = rtt_rx.recv().await {
            notifier_rtt.emit_network_rtt(rtt_ms);
        }
    });

    let (latency_tx, mut latency_rx) = tokio::sync::mpsc::channel::<(f32, f32, f32)>(10);
    tokio::spawn(async move {
        while let Some((latency, rms, jitter)) = latency_rx.recv().await {
            notifier.emit_audio_telemetry(latency, rms > 0.0001, jitter);
            // println!("Latency: {:.2}ms RMS: {:.2}", latency, rms);
        }
    });

    (streamer_ip_tx, latency_tx, rtt_tx)
}

pub type SessionPlayerResult = Result<
    (
        Arc<AtomicBool>,
        Arc<AtomicBool>,
        Arc<RwLock<JitterConfig>>,
        Arc<AtomicU32>,
        oneshot::Sender<()>,
        JoinHandle<()>,
        bool, // exclusive_granted
    ),
    String,
>;

#[allow(clippy::too_many_arguments)]
pub fn spawn_session_player(
    jitter_config: JitterConfig,
    is_tcp: bool,
    exclusive_mode: bool,
    notifier: Arc<dyn FrontendNotifier>,
    target_ip: Option<std::net::IpAddr>,
    mode: gemacast_core::domain::types::ConnectionMode,
    device_id: String,
    network_link: gemacast_core::domain::types::NetworkLink,
    session_token: Option<String>,
    session_generation: Option<gemacast_core::control::SessionGeneration>,
) -> SessionPlayerResult {
    let config_ref = Arc::new(RwLock::new(jitter_config));
    let is_tcp_mode = Arc::new(AtomicBool::new(is_tcp));
    let is_playing = Arc::new(AtomicBool::new(true));
    let volume = Arc::new(AtomicU32::new(f32::to_bits(1.0)));
    let (shutdown_tx, shutdown_rx) = oneshot::channel();

    let player = gemacast_core::stream::player::AudioStreamPlayer::new(
        config_ref.clone(),
        is_tcp_mode.clone(),
        network_link,
        is_playing.clone(),
        volume.clone(),
        exclusive_mode,
        shutdown_rx,
    )
    .map_err(|e| e.to_string())?;

    let exclusive_granted = player.exclusive_granted;

    let mut player = player;
    let (streamer_ip_tx, latency_tx, rtt_tx) = setup_event_forwarding(notifier.clone());

    let task = tokio::spawn(async move {
        if let Err(e) = player.activate_playback_stream() {
            notifier.emit_playback_error(e.to_string());
            return;
        }

        if let Err(e) = player
            .run_audio_receive_loop(
                Some(streamer_ip_tx),
                Some(latency_tx),
                Some(rtt_tx),
                target_ip,
                mode,
                gemacast_core::stream::player::AudioSessionCredentials {
                    device_id: gemacast_core::domain::types::DeviceId(device_id),
                    session_token,
                    session_generation,
                },
            )
            .await
        {
            if matches!(
                e,
                gemacast_core::domain::error::GemaCastError::Network(
                    gemacast_core::domain::error::NetworkError::ConnectionLost
                )
            ) {
                // The playback watchdog gave up on its own — nobody asked for
                // this. Distinct from `force-disconnect` so the frontend can
                // attempt probe-driven recovery instead of just going idle.
                notifier.emit_link_lost();
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
        exclusive_granted,
    ))
}
