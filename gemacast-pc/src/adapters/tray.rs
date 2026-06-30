use crate::events::TrayEvent;
use crate::traits::TrayNotifier;
use gemacast_core::domain::types::{DeviceId, TransportType};
use std::net::SocketAddr;
use tao::event_loop::EventLoopProxy;

/// Sends [`TrayEvent`]s to the tray event loop via `EventLoopProxy`.
pub struct EventLoopTrayNotifier {
    proxy: EventLoopProxy<TrayEvent>,
}

impl EventLoopTrayNotifier {
    pub fn new(proxy: EventLoopProxy<TrayEvent>) -> Self {
        Self { proxy }
    }
}

impl TrayNotifier for EventLoopTrayNotifier {
    fn notify_device_discovered(
        &self,
        device_id: DeviceId,
        name: String,
        addr: SocketAddr,
        transport: Option<TransportType>,
    ) {
        let _ = self.proxy.send_event(TrayEvent::DiscoveredDevice {
            device_id,
            name,
            addr,
            transport,
        });
    }

    fn notify_device_lost(&self, device_id: DeviceId, addr: SocketAddr) {
        let _ = self
            .proxy
            .send_event(TrayEvent::DeviceLost { device_id, addr });
    }

    fn notify_fatal_error(&self, message: String) {
        let _ = self.proxy.send_event(TrayEvent::FatalError(message));
    }

    fn notify_shutdown_complete(&self) {
        let _ = self.proxy.send_event(TrayEvent::ShutdownComplete);
    }

    fn notify_update_ready(&self, version: String, installer_path: std::path::PathBuf) {
        let _ = self.proxy.send_event(TrayEvent::UpdateReady {
            version,
            installer_path,
        });
    }

    fn notify_update_failed(&self, message: String) {
        let _ = self.proxy.send_event(TrayEvent::UpdateFailed(message));
    }

    fn notify_update_checking(&self) {
        let _ = self.proxy.send_event(TrayEvent::UpdateChecking);
    }

    fn notify_update_up_to_date(&self) {
        let _ = self.proxy.send_event(TrayEvent::UpdateUpToDate);
    }
}
