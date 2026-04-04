use gemacast_core::network::AudioReceiver;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{Emitter, State};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

struct AppState {
    discovery_handle: Mutex<Option<JoinHandle<()>>>,
    playback_handle: Mutex<Option<JoinHandle<()>>>,
    shutdown_playback_tx: Mutex<Option<oneshot::Sender<()>>>,
    is_playing: Mutex<Option<Arc<AtomicBool>>>,
    volume: Mutex<Option<Arc<AtomicU32>>>,
}

#[tauri::command]
fn get_local_ip() -> Result<String, String> {
    gemacast_core::network::get_local_ip()
        .map(|ip| ip.to_string())
        .map_err(|e| e.to_string())
}

fn stop_audio_playback_inner(state: &AppState) -> Result<(), String> {
    if let Some(tx) = state
        .shutdown_playback_tx
        .lock()
        .map_err(|e| e.to_string())?
        .take()
    {
        let _ = tx.send(());
    }
    if let Some(handle) = state
        .playback_handle
        .lock()
        .map_err(|e| e.to_string())?
        .take()
    {
        handle.abort();
    }
    let _ = state.is_playing.lock().map_err(|e| e.to_string())?.take();
    let _ = state.volume.lock().map_err(|e| e.to_string())?.take();
    Ok(())
}

fn setup_event_forwarding(
    app_handle: tauri::AppHandle,
) -> (oneshot::Sender<String>, tokio::sync::mpsc::Sender<f32>) {
    let (sender_ip_tx, sender_ip_rx) = oneshot::channel::<String>();
    let handle_conn = app_handle.clone();
    tokio::spawn(async move {
        if let Ok(ip) = sender_ip_rx.await {
            let _ = handle_conn.emit("sender-connected", ip);
        }
    });

    let (latency_tx, mut latency_rx) = tokio::sync::mpsc::channel::<f32>(10);
    let handle_latency = app_handle.clone();
    tokio::spawn(async move {
        while let Some(latency) = latency_rx.recv().await {
            let _ = handle_latency.emit("latency-update", latency);
        }
    });

    (sender_ip_tx, latency_tx)
}

fn spawn_playback_task(
    mut receiver: AudioReceiver,
    app_handle: tauri::AppHandle,
    sender_ip_tx: oneshot::Sender<String>,
    latency_tx: tokio::sync::mpsc::Sender<f32>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        if let Err(e) = receiver.start_audio_playback() {
            eprintln!("Playback start failed: {:?}", e);
            let _ = app_handle.emit("playback-error", e.to_string());
            return;
        }

        if let Err(e) = receiver
            .start_audio_listener(Some(sender_ip_tx), Some(latency_tx))
            .await
        {
            eprintln!("Audio listener task crashed: {:?}", e);
            let _ = app_handle.emit("playback-error", e.to_string());
        }
    })
}

fn spawn_discovery_listener(
    listener: gemacast_core::network::DiscoveryListener,
    mut discovery_rx: tokio::sync::mpsc::Receiver<(
        gemacast_core::types::ControlMessage,
        std::net::SocketAddr,
    )>,
    app_handle: tauri::AppHandle,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let listener_handle = tokio::spawn(async move {
            if let Err(e) = listener.start().await {
                eprintln!("Discovery listener crashed: {:?}", e);
            }
        });

        while let Some((message, addr)) = discovery_rx.recv().await {
            match message {
                gemacast_core::types::ControlMessage::Presence {
                    sender_id,
                    sender_name,
                    is_offline,
                } => {
                    let mut audio_addr = addr;
                    audio_addr.set_port(gemacast_core::network::AUDIO_PORT);
                    let device = gemacast_core::types::DiscoveredDevice::from_presence(
                        sender_id,
                        sender_name,
                        is_offline,
                        audio_addr,
                    );
                    let _ = app_handle.emit("sender-discovered", device);
                }
                gemacast_core::types::ControlMessage::Disconnect { .. } => {
                    let _ = app_handle.emit("force-disconnect", ());
                }
                _ => {}
            }
        }
        listener_handle.abort();
    })
}

#[tauri::command]
async fn start_listening_for_senders(
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    if let Some(handle) = state
        .discovery_handle
        .lock()
        .map_err(|e| e.to_string())?
        .take()
    {
        handle.abort();
    }

    let gemacast_core::network::DiscoveryListenerHandles {
        listener,
        discovery_rx,
    } = gemacast_core::network::DiscoveryListener::new();

    let handle = spawn_discovery_listener(listener, discovery_rx, app_handle);
    *state.discovery_handle.lock().map_err(|e| e.to_string())? = Some(handle);
    Ok(())
}

#[tauri::command]
async fn stop_listening_for_senders(state: State<'_, AppState>) -> Result<(), String> {
    if let Some(handle) = state
        .discovery_handle
        .lock()
        .map_err(|e| e.to_string())?
        .take()
    {
        handle.abort();
    }
    Ok(())
}

#[tauri::command]
async fn connect_to_sender(
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
            device_id,
            device_name,
        },
    )
    .await
    .map_err(|e| e.to_string())?;

    stop_audio_playback_inner(&state)?;

    let audio_handles = AudioReceiver::create().await.map_err(|e| e.to_string())?;
    *state
        .shutdown_playback_tx
        .lock()
        .map_err(|e| e.to_string())? = Some(audio_handles.shutdown_tx);
    *state.is_playing.lock().map_err(|e| e.to_string())? = Some(audio_handles.is_playing);
    *state.volume.lock().map_err(|e| e.to_string())? = Some(audio_handles.volume);

    let (sender_ip_tx, latency_tx) = setup_event_forwarding(app_handle.clone());
    let playback_task =
        spawn_playback_task(audio_handles.receiver, app_handle, sender_ip_tx, latency_tx);
    *state.playback_handle.lock().map_err(|e| e.to_string())? = Some(playback_task);

    Ok(())
}

#[tauri::command]
async fn disconnect_from_sender(
    ip: String,
    device_id: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let ip_addr = ip.parse::<std::net::IpAddr>().map_err(|e| e.to_string())?;
    let _ = gemacast_core::network::send_control_message(
        ip_addr,
        gemacast_core::types::ControlMessage::Disconnect { device_id },
    )
    .await;

    stop_audio_playback_inner(&state)?;
    Ok(())
}

#[tauri::command]
async fn stop_audio_playback(state: State<'_, AppState>) -> Result<(), String> {
    let is_playing_lock = state.is_playing.lock().map_err(|e| e.to_string())?;
    if let Some(is_playing) = is_playing_lock.as_ref() {
        is_playing.store(false, Ordering::Relaxed);
    }
    Ok(())
}

#[tauri::command]
async fn start_audio_playback(state: State<'_, AppState>) -> Result<(), String> {
    let is_playing_lock = state.is_playing.lock().map_err(|e| e.to_string())?;
    if let Some(is_playing) = is_playing_lock.as_ref() {
        is_playing.store(true, Ordering::Relaxed);
    }
    Ok(())
}

#[tauri::command]
async fn set_volume(level: f32, state: State<'_, AppState>) -> Result<(), String> {
    let clamped = level.clamp(0.0, 1.0);
    let vol_lock = state.volume.lock().map_err(|e| e.to_string())?;
    if let Some(vol) = vol_lock.as_ref() {
        vol.store(f32::to_bits(clamped), Ordering::Relaxed);
    }
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(AppState {
            discovery_handle: Mutex::new(None),
            playback_handle: Mutex::new(None),
            shutdown_playback_tx: Mutex::new(None),
            is_playing: Mutex::new(None),
            volume: Mutex::new(None),
        })
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_device_info::init())
        .invoke_handler(tauri::generate_handler![
            start_listening_for_senders,
            stop_listening_for_senders,
            connect_to_sender,
            disconnect_from_sender,
            get_local_ip,
            start_audio_playback,
            stop_audio_playback,
            set_volume
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
