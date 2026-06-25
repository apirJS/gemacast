use async_trait::async_trait;
use gemacast_core::domain::types::DeviceId;
use std::net::SocketAddr;

/// Notifies a connected device to disconnect, using the best available transport.
///
/// The notification order is: WebSocket → ADB broadcast (if loopback) → HTTP (if remote).
///
/// **Production**: [`crate::adapters::MultiTransportDeviceNotifier`].
/// **Tests**: [`crate::testing::mocks::MockDeviceNotifier`] that records calls.
#[async_trait]
pub trait DeviceNotifier: Send + Sync {
    /// Notify a device to disconnect using all available transports.
    ///
    /// `addr` is used to choose between ADB (loopback) and HTTP (remote) fallback
    /// when WebSocket notification fails. Pass `None` to skip fallback.
    async fn notify_disconnect(&self, device_id: &DeviceId, addr: Option<SocketAddr>);

    /// Signal all ADB TCP tasks to tear down their connections.
    fn signal_adb_shutdown(&self);
}
