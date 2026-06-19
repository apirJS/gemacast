use crate::traits::types::{SessionInfo, SessionParams};
use async_trait::async_trait;
use gemacast_core::types::JitterConfig;

/// Manages the lifecycle of audio playback sessions and WebSocket clients.
///
/// Encapsulates all `ActiveSession` state, `JoinHandle` tracking,
/// and shutdown signaling.
///
/// **Production**: [`crate::adapters::TokioSessionManager`]
/// **Tests**: [`crate::testing::mocks::MockSessionManager`]
#[async_trait]
pub trait SessionManager: Send + Sync {
    /// Tear down any existing session, then spawn a new audio receiver.
    async fn start_session(&self, params: SessionParams) -> Result<(), String>;

    /// Gracefully shut down the active session and WebSocket client.
    async fn stop_session(&self);

    /// Set the is_playing flag on the active session.
    async fn set_playing(&self, playing: bool);

    /// Pause the audio output stream without tearing down the session.
    ///
    /// Sets `is_playing` to `false` so the Oboe callback outputs silence,
    /// but keeps the network receive thread, heartbeat, and WebSocket alive.
    async fn pause_playback(&self) -> Result<(), String>;

    /// Resume the audio output stream after a pause.
    ///
    /// Sets `is_playing` to `true` so the Oboe callback resumes normal
    /// playback from the jitter buffer.
    async fn resume_playback(&self) -> Result<(), String>;

    /// Update the jitter configuration on the active session.
    async fn update_jitter_config(&self, config: JitterConfig);

    /// Get a snapshot of the active session's metadata.
    async fn session_info(&self) -> Option<SessionInfo>;

    /// Update the stored bitrate on the active session.
    async fn update_bitrate(&self, bitrate: Option<i32>);

    /// Set the audio output volume as a linear multiplier (1.0 = unity gain).
    async fn set_volume(&self, linear: f32);

    /// Track a WebSocket client task. Aborts any previous WS task.
    async fn start_ws_client(&self, task: tokio::task::JoinHandle<()>);

    /// Abort the tracked WebSocket client task.
    async fn stop_ws_client(&self);
}
