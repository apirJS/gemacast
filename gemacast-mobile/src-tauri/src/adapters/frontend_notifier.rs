use crate::traits::FrontendNotifier;
use gemacast_core::domain::types::{DeviceId, DiscoveredDevice};
use tauri::Emitter;

/// Emits events to the Tauri webview frontend via `AppHandle::emit()`.
pub struct TauriFrontendNotifier {
    app_handle: tauri::AppHandle,
}

impl TauriFrontendNotifier {
    pub fn new(app_handle: tauri::AppHandle) -> Self {
        Self { app_handle }
    }
}

impl FrontendNotifier for TauriFrontendNotifier {
    fn emit_sender_discovered(&self, device: DiscoveredDevice) {
        let _ = self.app_handle.emit("sender-discovered", device);
    }

    fn emit_sender_timeout(&self, sender_id: &DeviceId) {
        let _ = self.app_handle.emit("sender-timeout", sender_id.0.clone());
    }

    fn emit_force_disconnect(&self) {
        let _ = self.app_handle.emit("force-disconnect", ());
    }

    fn emit_sender_connected(&self, ip: String) {
        let _ = self.app_handle.emit("sender-connected", ip);
    }

    fn emit_audio_telemetry(&self, latency: f32, is_active: bool) {
        #[derive(serde::Serialize, Clone)]
        #[serde(rename_all = "camelCase")]
        struct AudioTelemetry {
            latency: f32,
            is_active: bool,
        }
        let _ = self
            .app_handle
            .emit("audio-telemetry", AudioTelemetry { latency, is_active });
    }

    fn emit_playback_error(&self, error: String) {
        let _ = self.app_handle.emit("playback-error", error);
    }

    fn emit_ws_disconnect(&self) {
        let _ = self.app_handle.emit("ws-disconnect", ());
    }

    fn emit_ws_error(&self, message: String) {
        let _ = self.app_handle.emit("ws-error", message);
    }

    fn emit_service_command(&self, command: String) {
        let _ = self.app_handle.emit("service-command", command);
    }
}
