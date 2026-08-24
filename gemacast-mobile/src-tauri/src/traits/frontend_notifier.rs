use gemacast_core::domain::types::{DeviceId, DiscoveredDevice};

/// Emits events to the web frontend.
///
/// Abstracts away `tauri::AppHandle::emit()` so domain logic never depends
/// on the Tauri runtime.
///
/// **Production**: [`crate::adapters::TauriFrontendNotifier`]
/// **Tests**: [`crate::testing::mocks::MockFrontendNotifier`]
pub trait FrontendNotifier: Send + Sync {
    /// A streamer was discovered or updated on the network.
    fn emit_streamer_discovered(&self, device: DiscoveredDevice);

    /// A streamer's heartbeat timed out.
    fn emit_streamer_timeout(&self, streamer_id: &DeviceId);

    /// The streamer forcibly disconnected us.
    fn emit_force_disconnect(&self);

    /// The audio link went silent and the playback watchdog tore the session
    /// down on its own — nobody asked for this disconnect.
    ///
    /// Deliberately distinct from [`Self::emit_force_disconnect`], which is
    /// also the *user-initiated* teardown path: the frontend must be able to
    /// tell "the PC told us to stop" from "the packets stopped arriving", and
    /// only attempt recovery for the latter.
    fn emit_link_lost(&self);

    /// A recovery probe reached the PC again after a link loss.
    ///
    /// `device_registered` is the PC's answer to "do you still have us in the
    /// registry?" — `Some(true)` means the PC never evicted us, `Some(false)`
    /// means it did, `None` means an older streamer that cannot say. It is
    /// carried for observability: today every answer takes the same full
    /// reconnect, and a field capture is what would justify a cheaper path.
    fn emit_link_recovered(&self, device_registered: Option<bool>);

    /// Link recovery exhausted its budget without reaching the PC.
    ///
    /// The session stays torn down and suspended — exactly today's behaviour
    /// after a link loss. This exists so the give-up is visible rather than
    /// looking like a prober that silently kept running.
    fn emit_link_recovery_gave_up(&self);

    /// Successfully connected to a streamer's audio stream.
    fn emit_streamer_connected(&self, ip: String);

    /// Periodic audio telemetry update.
    ///
    /// `latency` is the buffer dwell time in ms (a frame's time between arrival
    /// and playback); `jitter_ms` is the rolling network arrival-jitter estimate
    /// in ms. Two distinct signals — see the jitter buffer's `latency_metric` /
    /// `jitter_metric`.
    fn emit_audio_telemetry(&self, latency: f32, is_active: bool, jitter_ms: f32);

    /// An error occurred during audio playback.
    fn emit_playback_error(&self, error: String);

    /// Raw wire round-trip time (ms) from the UDP echo ping.
    ///
    /// The phone piggybacks a timestamped ping on its keepalive heartbeat
    /// (~every 500 ms); the PC reflects it and the player reports the
    /// round-trip here. Measures the real UDP path latency, not a TLS
    /// handshake. UDP-only: ADB/loopback runs over TCP with no echo, so the
    /// frontend shows `--` there. Independent of
    /// [`Self::emit_audio_telemetry`]'s cadence.
    fn emit_network_rtt(&self, rtt_ms: f32);

    /// The WebSocket control connection was closed.
    fn emit_ws_disconnect(&self);

    /// An error occurred on the WebSocket control connection.
    fn emit_ws_error(&self, message: String);

    /// An IPC service command was received from the Android service.
    fn emit_service_command(&self, command: String);
}
