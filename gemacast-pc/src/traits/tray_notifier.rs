use gemacast_core::domain::types::{DeviceId, TransportType};
use std::net::SocketAddr;

/// Sends UI events to the tray event loop.
///
/// **Production**: [`crate::adapters::EventLoopTrayNotifier`] wrapping `EventLoopProxy<TrayEvent>`.
/// **Tests**: [`crate::testing::mocks::MockTrayNotifier`] that records calls.
pub trait TrayNotifier: Send + Sync {
    /// A new device connected (or reconnected at a new address).
    fn notify_device_discovered(
        &self,
        device_id: DeviceId,
        name: String,
        addr: SocketAddr,
        transport: Option<TransportType>,
    );

    /// A device disconnected or was evicted.
    fn notify_device_lost(&self, device_id: DeviceId, addr: SocketAddr);

    /// An unrecoverable background error occurred.
    fn notify_fatal_error(&self, message: String);

    /// The background engine has finished shutting down.
    fn notify_shutdown_complete(&self);
}
