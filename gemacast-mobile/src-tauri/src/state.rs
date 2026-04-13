use std::net::IpAddr;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, MutexGuard};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

/// Acquires a [`Mutex`] lock and maps the poison error to a [`String`]
pub fn lock<T>(m: &Mutex<T>) -> Result<MutexGuard<'_, T>, String> {
    m.lock().map_err(|e| e.to_string())
}

/// Shared application state managed by Tauri.
pub struct AppState {
    /// Handle to the running discovery listener task.
    pub discovery_handle: Mutex<Option<JoinHandle<()>>>,

    /// Handle to the running audio playback task.
    pub playback_handle: Mutex<Option<JoinHandle<()>>>,

    /// Oneshot sender used to cleanly shut down the audio receiver.
    pub shutdown_playback_tx: Mutex<Option<oneshot::Sender<()>>>,

    /// Atomic flag shared with the audio thread — `true` while audio should play.
    pub is_playing: Mutex<Option<Arc<AtomicBool>>>,

    /// IP address of the currently connected PC sender.
    pub connected_ip: Mutex<Option<IpAddr>>,

    /// Stable device identifier sent in control messages.
    pub device_id: Mutex<Option<String>>,

    /// Human-readable device name sent in control messages.
    pub device_name: Mutex<Option<String>>,

    /// Shared JitterConfig reference capable of dynamic runtime updates.
    pub config_ref: Mutex<Option<Arc<std::sync::RwLock<gemacast_core::types::JitterConfig>>>>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            discovery_handle: Mutex::new(None),
            playback_handle: Mutex::new(None),
            shutdown_playback_tx: Mutex::new(None),
            is_playing: Mutex::new(None),
            connected_ip: Mutex::new(None),
            device_id: Mutex::new(None),
            device_name: Mutex::new(None),
            config_ref: Mutex::new(None),
        }
    }
}
