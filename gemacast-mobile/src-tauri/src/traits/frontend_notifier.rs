use gemacast_core::domain::types::{DeviceId, DiscoveredDevice};

/// Emits events to the web frontend.
///
/// Abstracts away `tauri::AppHandle::emit()` so domain logic never depends
/// on the Tauri runtime.
///
/// **Production**: [`crate::adapters::TauriFrontendNotifier`]
/// **Tests**: [`crate::testing::mocks::MockFrontendNotifier`]
pub trait FrontendNotifier: Send + Sync {
    /// A sender was discovered or updated on the network.
    fn emit_sender_discovered(&self, device: DiscoveredDevice);

    /// A sender's heartbeat timed out.
    fn emit_sender_timeout(&self, sender_id: &DeviceId);

    /// The sender forcibly disconnected us.
    fn emit_force_disconnect(&self);

    /// Successfully connected to a sender's audio stream.
    fn emit_sender_connected(&self, ip: String);

    /// Periodic audio telemetry update.
    fn emit_audio_telemetry(&self, latency: f32, is_active: bool);

    /// An error occurred during audio playback.
    fn emit_playback_error(&self, error: String);

    /// The WebSocket control connection was closed.
    fn emit_ws_disconnect(&self);

    /// An error occurred on the WebSocket control connection.
    fn emit_ws_error(&self, message: String);

    /// An IPC service command was received from the Android service.
    fn emit_service_command(&self, command: String);
}
