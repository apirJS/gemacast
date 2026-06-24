//! Thin Tauri command wrappers that delegate to pure service functions.
//!
//! Each `#[tauri::command]` handler extracts dependencies from
//! [`crate::state::AppState`] and delegates to [`super::service`].

use gemacast_core::domain::types::{ConnectionMode, DeviceId};
use tauri::State;

use crate::state::AppState;

use super::listener::spawn_discovery_listener;

#[tauri::command]
pub fn get_local_ip(state: State<'_, AppState>) -> Result<String, String> {
    super::service::get_local_ip(state.network.as_ref())
}

#[tauri::command]
pub fn get_network_identifier(state: State<'_, AppState>) -> Result<String, String> {
    super::service::get_network_identifier(state.network.as_ref())
}

#[tauri::command]
pub async fn start_listening_for_senders(
    device_id: DeviceId,
    mode: ConnectionMode,
    state: State<'_, AppState>,
) -> Result<(), String> {
    if let Some(handle) = state.discovery_task.lock().await.take() {
        handle.abort();
    }

    let (presence_message_tx, presence_message_rx) = tokio::sync::mpsc::channel(8);

    // Spawn mDNS listener
    let mdns_tx = presence_message_tx.clone();
    tokio::spawn(async move {
        if let Err(e) = gemacast_core::discovery::MdnsListener::run(mdns_tx).await {
            tracing::warn!("mDNS listener failed or not available: {}", e);
        }
    });

    let listener = gemacast_core::network::PresenceListener::new(presence_message_tx)
        .await
        .map_err(|e| {
            let e_str = e.to_string();
            if e_str.contains("Address already in use")
                || e_str.contains("10048")
                || e_str.contains("98")
                || e_str.contains("WSAEADDRINUSE")
            {
                "Discovery port is already in use. Is GemaCast already running in the background?"
                    .to_string()
            } else {
                e_str
            }
        })?;

    let handle = spawn_discovery_listener(
        listener,
        presence_message_rx,
        state.notifier.clone(),
        device_id,
        mode,
    );
    *state.discovery_task.lock().await = Some(handle);
    Ok(())
}

#[tauri::command]
pub async fn stop_listening_for_senders(state: State<'_, AppState>) -> Result<(), String> {
    if let Some(handle) = state.discovery_task.lock().await.take() {
        handle.abort();
    }
    Ok(())
}

#[tauri::command]
pub fn get_connection_status(
    state: State<'_, AppState>,
) -> Result<gemacast_core::domain::types::ConnectionModes, String> {
    super::service::get_connection_status(state.network.as_ref(), state.platform.as_ref())
}

#[tauri::command]
pub fn get_network_state(
    state: State<'_, AppState>,
) -> Result<super::service::NetworkState, String> {
    super::service::get_network_state(state.network.as_ref(), state.platform.as_ref())
}
