//! Application state managed by Tauri.
//!
//! After the Hexagonal refactor, `AppState` is a simple container for
//! `Arc<dyn Trait>` objects and the `AudioService`. All session state
//! lives inside `TokioSessionManager` (the production [`SessionManager`] adapter).

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use tokio::sync::Mutex;
use tokio::task::JoinHandle;

use crate::domains::audio::service::AudioService;
use crate::traits::{FrontendNotifier, NetworkInfoProvider, PlatformService};

pub struct AppState {
    pub audio: Arc<AudioService>,
    pub notifier: Arc<dyn FrontendNotifier>,
    pub network: Arc<dyn NetworkInfoProvider>,
    pub platform: Arc<dyn PlatformService>,
    pub discovery_task: Mutex<Option<JoinHandle<()>>>,
    /// Shared flag: `true` while an audio session is active.
    /// The probe loop checks this to skip subnet scans during streaming,
    /// preventing 254 UDP packets from flooding the 2.4 GHz channel.
    pub is_streaming: Arc<AtomicBool>,
}
