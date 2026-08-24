//! Adapter: WebSocket-based error notification.
//!
//! Production implementation of [`ErrorNotifier`](crate::ports::error_notifier::ErrorNotifier)
//! that sends `WsEvent::Error` to connected players via the shared WebSocket
//! connection map.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;

use crate::control::types::WsEvent;
use crate::domain::types::DeviceId;
use crate::ports::error_notifier::ErrorNotifier;

/// Type alias for the shared WebSocket connection map.
///
/// Maps `DeviceId` → `mpsc::Sender<WsEvent>` for each active WebSocket connection.
pub type WsConnectionMap = Arc<Mutex<HashMap<DeviceId, mpsc::Sender<WsEvent>>>>;

/// Notifies connected players about engine errors via WebSocket.
///
/// Uses `try_send` (non-blocking) to avoid stalling the engine's command loop
/// if a WebSocket consumer is slow. Dropped notifications are acceptable for
/// error messages — the player will detect the issue independently via
/// network timeout or heartbeat failure.
#[derive(Clone)]
pub struct WsErrorNotifier {
    ws_connections: WsConnectionMap,
}

impl WsErrorNotifier {
    pub fn new(ws_connections: WsConnectionMap) -> Self {
        Self { ws_connections }
    }
}

impl ErrorNotifier for WsErrorNotifier {
    fn notify_error(&self, device_id: &DeviceId, message: String) {
        let ws_tx = {
            let connections = self.ws_connections.lock().unwrap();
            connections.get(device_id).cloned()
        };

        if let Some(tx) = ws_tx {
            let _ = tx.try_send(WsEvent::Error { message });
        }
    }
}
