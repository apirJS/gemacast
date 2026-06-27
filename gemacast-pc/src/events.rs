//! Events and commands exchanged between the tray UI and the background engine.
//!
//! - [`TrayEvent`]: Sent *from* background tasks *to* the tray event loop.
//! - [`AppCommand`]: Sent *from* the tray UI *to* the background engine.

use gemacast_core::domain::types::{DeviceId, TransportType};
use std::net::SocketAddr;

/// Events sent to the tray UI from background tasks.
///
/// These drive the system tray menu: adding/removing device entries
/// and displaying fatal error dialogs.
pub enum TrayEvent {
    /// A new version has been downloaded and is ready to install.
    UpdateReady {
        version: String,
        installer_path: std::path::PathBuf,
    },
    /// An update check or download failed.
    UpdateFailed(String),
    /// A new device connected or an existing device changed its IP.
    DiscoveredDevice {
        device_id: DeviceId,
        name: String,
        addr: SocketAddr,
        transport: Option<TransportType>,
    },
    /// A device disconnected or was evicted by the watchdog.
    DeviceLost {
        device_id: DeviceId,
        #[allow(dead_code)]
        addr: SocketAddr,
    },
    /// An unrecoverable error occurred in the background engine.
    FatalError(String),
    /// The OS or user requested a process shutdown (e.g. Ctrl+C).
    ShutdownRequested,
    /// The background engine has finished tearing down resources.
    ShutdownComplete,
}

/// Commands sent from the tray UI to the background engine.
///
/// Processed by the [`CommandHandler`](crate::tasks::command_handler::CommandHandler).
#[derive(Debug, Clone)]
pub enum AppCommand {
    /// Disconnect a specific device and stop streaming to it.
    KickDevice(DeviceId),
    /// Gracefully shut down all streams and disconnect all devices.
    StopAllStreams,
    /// Begin broadcasting presence and accepting connections.
    StartBroadcasting,
    /// Stop broadcasting and unsubscribe all devices from audio.
    StopBroadcasting,
    /// Trigger a complete shutdown of the background engine.
    ExitApp,
}
