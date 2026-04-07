use tauri::State;

use crate::discovery::spawn_discovery_listener;
use crate::state::{lock, AppState};

/// Returns the device's primary non-loopback IPv4 address as a string.
#[tauri::command]
pub fn get_local_ip() -> Result<String, String> {
    gemacast_core::network::get_local_ip()
        .map(|ip| ip.to_string())
        .map_err(|e| e.to_string())
}

/// Starts the discovery listener, replacing any currently running one.
///
/// Spawns the full discovery pipeline (listener + watchdog + USB probe)
/// and stores the task handle in [`AppState`].
#[tauri::command]
pub async fn start_listening_for_senders(
    device_id: String,
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    if let Some(handle) = lock(&state.discovery_handle)?.take() {
        handle.abort();
    }

    let gemacast_core::network::DiscoveryListenerHandles {
        listener,
        discovery_rx,
    } = gemacast_core::network::DiscoveryListener::new()
        .await
        .map_err(|e| e.to_string())?;

    let handle = spawn_discovery_listener(listener, discovery_rx, app_handle, device_id);
    *lock(&state.discovery_handle)? = Some(handle);
    Ok(())
}

/// Stops the currently running discovery listener, if any.
#[tauri::command]
pub async fn stop_listening_for_senders(state: State<'_, AppState>) -> Result<(), String> {
    if let Some(handle) = lock(&state.discovery_handle)?.take() {
        handle.abort();
    }
    Ok(())
}
