use async_trait::async_trait;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use tokio::sync::{broadcast, mpsc};

use gemacast_core::control::HttpControlClient;
use gemacast_core::control::http::send_ws_event;
use gemacast_core::control::types::WsEvent;
use gemacast_core::types::{ControlMessage, DeviceId};

use crate::traits::DeviceNotifier;

/// WebSocket connection map type alias for readability.
pub type WsConnectionMap = Arc<Mutex<HashMap<DeviceId, mpsc::Sender<WsEvent>>>>;

/// Notifies devices to disconnect using the best available transport.
///
/// Priority: WebSocket → ADB broadcast (loopback) → HTTP (remote).
pub struct MultiTransportDeviceNotifier {
    ws_connections: WsConnectionMap,
    adb_outbound_control: broadcast::Sender<ControlMessage>,
    adb_shutdown: broadcast::Sender<()>,
}

impl MultiTransportDeviceNotifier {
    pub fn new(
        ws_connections: WsConnectionMap,
        adb_outbound_control: broadcast::Sender<ControlMessage>,
        adb_shutdown: broadcast::Sender<()>,
    ) -> Self {
        Self {
            ws_connections,
            adb_outbound_control,
            adb_shutdown,
        }
    }
}

#[async_trait]
impl DeviceNotifier for MultiTransportDeviceNotifier {
    async fn notify_disconnect(&self, device_id: &DeviceId, addr: Option<SocketAddr>) {
        // Try WebSocket first
        let ws_ok = send_ws_event(&self.ws_connections, device_id, WsEvent::Disconnect)
            .await
            .is_ok();

        // Fallback if WebSocket didn't reach the device
        if !ws_ok && let Some(addr) = addr {
            if addr.ip().is_loopback() {
                let _ = self.adb_outbound_control.send(ControlMessage::Disconnect {
                    device_id: device_id.clone(),
                });
            } else {
                let client = HttpControlClient::new(addr.ip());
                let _ = client.send_disconnect_request(device_id.clone()).await;
            }
        }
    }

    fn signal_adb_shutdown(&self) {
        let _ = self.adb_shutdown.send(());
    }
}
