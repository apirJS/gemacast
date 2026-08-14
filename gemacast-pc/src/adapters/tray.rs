use crate::events::TrayEvent;
use crate::traits::TrayNotifier;
use async_trait::async_trait;
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

#[async_trait]
impl TrayNotifier for EventLoopTrayNotifier {
    async fn request_connection_approval(
        &self,
        request_id: String,
        _device_id: DeviceId,
        name: String,
        addr: SocketAddr,
        key_fingerprint: String,
        pairing_code: String,
    ) -> bool {
        let (response_tx, response_rx) = tokio::sync::oneshot::channel();
        if self
            .proxy
            .send_event(TrayEvent::ConnectionApproval {
                request_id,
                name,
                addr,
                key_fingerprint,
                pairing_code,
                response_tx,
            })
            .is_err()
        {
            return false;
        }

        tokio::time::timeout(std::time::Duration::from_secs(60), response_rx)
            .await
            .ok()
            .and_then(Result::ok)
            .unwrap_or(false)
    }

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
