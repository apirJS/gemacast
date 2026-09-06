use gemacast_core::domain::types::{DeviceId, DiscoveredDevice};

pub trait FrontendNotifier: Send + Sync {
    fn emit_streamer_discovered(&self, device: DiscoveredDevice);

    fn emit_streamer_timeout(&self, streamer_id: &DeviceId);

    fn emit_force_disconnect(&self);

    fn emit_link_lost(&self);

    fn emit_link_recovered(&self, device_registered: Option<bool>);

    fn emit_link_recovery_gave_up(&self);

    fn emit_streamer_connected(&self, ip: String);

    fn emit_audio_telemetry(&self, latency: f32, is_active: bool, jitter_ms: f32);

    fn emit_playback_error(&self, error: String);

    fn emit_network_rtt(&self, rtt_ms: f32);

    fn emit_ws_disconnect(&self);

    fn emit_ws_error(&self, message: String);

    fn emit_pc_volume_changed(&self, level: f32);

    fn emit_service_command(&self, command: String);
}
