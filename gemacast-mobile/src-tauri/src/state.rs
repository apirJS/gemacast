use std::net::IpAddr;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, RwLock};
use tokio::sync::{oneshot, Mutex};
use tokio::task::JoinHandle;

use gemacast_core::types::{DeviceId, JitterConfig};

pub struct ActiveSession {
    pub ip: IpAddr,
    pub device_id: DeviceId,
    pub device_name: String,
    pub exclusive_mode: bool,
    pub mode: gemacast_core::types::ConnectionMode,
    pub bitrate: Option<i32>,

    pub is_playing: Arc<AtomicBool>,
    pub jitter_config: Arc<RwLock<JitterConfig>>,

    pub shutdown_tx: oneshot::Sender<()>,
    pub playback_task: JoinHandle<()>,
}

pub struct AppState {
    pub session: Mutex<Option<ActiveSession>>,
    pub discovery_task: Mutex<Option<JoinHandle<()>>>,
    pub ws_client: Mutex<Option<Arc<Mutex<gemacast_core::control::WsControlClient>>>>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            session: Mutex::new(None),
            discovery_task: Mutex::new(None),
            ws_client: Mutex::new(None),
        }
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}
