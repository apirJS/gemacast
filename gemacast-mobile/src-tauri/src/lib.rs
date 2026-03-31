use gemacast_core::network::{AudioReceiver, DiscoveryBroadcaster};
use gemacast_core::types::BroadcastPayload;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use tauri::{Emitter, State};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

struct AppState {
    discovery_handle: Mutex<Option<JoinHandle<()>>>,
    playback_handle: Mutex<Option<JoinHandle<()>>>,
    shutdown_discovery_tx: Mutex<Option<oneshot::Sender<()>>>,
    shutdown_playback_tx: Mutex<Option<oneshot::Sender<()>>>,
    is_playing: Mutex<Option<Arc<AtomicBool>>>,
}

#[tauri::command]
fn get_local_ip() -> Result<String, String> {
    gemacast_core::network::get_local_ip()
        .map(|ip| ip.to_string())
        .map_err(|e| e.to_string())
}

fn shutdown_task(
    tx_mutex: &Mutex<Option<oneshot::Sender<()>>>,
    handle_mutex: &Mutex<Option<JoinHandle<()>>>,
) -> Result<(), String> {
    if let Some(tx) = tx_mutex.lock().map_err(|e| e.to_string())?.take() {
        let _ = tx.send(());
    }
    if let Some(handle) = handle_mutex.lock().map_err(|e| e.to_string())?.take() {
        handle.abort();
    }
    Ok(())
}

fn stop_discovery_beacon_inner(state: &AppState) -> Result<(), String> {
    shutdown_task(&state.shutdown_discovery_tx, &state.discovery_handle)?;
    shutdown_task(&state.shutdown_playback_tx, &state.playback_handle)?;

    let _ = state.is_playing.lock().map_err(|e| e.to_string())?.take();

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

fn spawn_broadcast_task(
    broadcaster: DiscoveryBroadcaster,
    payload: BroadcastPayload,
    app_handle: tauri::AppHandle,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        if let Err(e) = broadcaster.broadcast_device(payload).await {
            eprintln!("Announcer task crashed: {:?}", e);
            let _ = app_handle.emit("discovery-error", e.to_string());
        }
    })
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

        if let Err(e) = receiver.start_audio_listener(Some(sender_ip_tx), Some(latency_tx)).await {
            eprintln!("Audio listener task crashed: {:?}", e);
            let _ = app_handle.emit("playback-error", e.to_string());
        }
    })
}

#[tauri::command]
async fn start_discovery_beacon(
    payload: BroadcastPayload,
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    stop_discovery_beacon_inner(&state)?;

    let broadcast_handles = DiscoveryBroadcaster::new()
        .await
        .map_err(|e| e.to_string())?;
    let audio_handles = AudioReceiver::create().await.map_err(|e| e.to_string())?;

    *state.shutdown_discovery_tx.lock().map_err(|e| e.to_string())? = Some(broadcast_handles.shutdown_tx);
    *state.shutdown_playback_tx.lock().map_err(|e| e.to_string())? = Some(audio_handles.shutdown_tx);
    *state.is_playing.lock().map_err(|e| e.to_string())? = Some(audio_handles.is_playing);

    let (sender_ip_tx, latency_tx) = setup_event_forwarding(app_handle.clone());
    let broadcast_task = spawn_broadcast_task(broadcast_handles.broadcaster, payload, app_handle.clone());
    let playback_task = spawn_playback_task(audio_handles.receiver, app_handle, sender_ip_tx, latency_tx);

    *state.discovery_handle.lock().map_err(|e| e.to_string())? = Some(broadcast_task);
    *state.playback_handle.lock().map_err(|e| e.to_string())? = Some(playback_task);

    Ok(())
}

#[tauri::command]
async fn stop_discovery_beacon(state: State<'_, AppState>) -> Result<(), String> {
    stop_discovery_beacon_inner(&state)
}

#[tauri::command]
async fn stop_audio_playback(state: State<'_, AppState>) -> Result<(), String> {
    let is_playing_lock = state.is_playing.lock().map_err(|e| e.to_string())?;
    if let Some(is_playing) = is_playing_lock.as_ref() {
        is_playing.store(false, std::sync::atomic::Ordering::Relaxed);
    }

    Ok(())
}

#[tauri::command]
async fn start_audio_playback(state: State<'_, AppState>) -> Result<(), String> {
    let is_playing_lock = state.is_playing.lock().map_err(|e| e.to_string())?;
    if let Some(is_playing) = is_playing_lock.as_ref() {
        is_playing.store(true, std::sync::atomic::Ordering::Relaxed);
    }

    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(AppState {
            discovery_handle: Mutex::new(None),
            playback_handle: Mutex::new(None),
            shutdown_discovery_tx: Mutex::new(None),
            shutdown_playback_tx: Mutex::new(None),
            is_playing: Mutex::new(None),
        })
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_device_info::init())
        .invoke_handler(tauri::generate_handler![
            start_discovery_beacon,
            stop_discovery_beacon,
            get_local_ip,
            start_audio_playback,
            stop_audio_playback
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
