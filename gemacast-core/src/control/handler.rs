use async_trait::async_trait;

use crate::error::GemaCastError;
use crate::types::{
    AudioSource, ConnectionMode, DeviceId, JitterConfig, SenderCapabilities,
};

/// Abstraction over the control channel between sender and receiver.
///
/// The sender side implements this to handle incoming requests.
/// The receiver side calls these methods to communicate with the sender.
///
/// Current implementation: UDP+JSON unicast (control/udp.rs)
/// Future implementation: Axum HTTP REST (control/http.rs)
#[async_trait]
pub trait ControlHandler: Send + Sync {
    /// Handle an incoming Connect request from a receiver.
    async fn handle_connect(
        &self,
        device_id: DeviceId,
        device_name: String,
        source: AudioSource,
        mode: ConnectionMode,
        jitter_config: JitterConfig,
    ) -> Result<(), GemaCastError>;

    /// Handle an incoming Disconnect request.
    async fn handle_disconnect(
        &self,
        device_id: DeviceId,
    ) -> Result<(), GemaCastError>;

    /// Return the list of available audio sources on this sender.
    async fn get_sources(&self) -> Result<(Vec<AudioSource>, SenderCapabilities), GemaCastError>;

    /// Handle a source change request from an already-connected receiver.
    async fn handle_change_source(
        &self,
        device_id: DeviceId,
        source: AudioSource,
    ) -> Result<(), GemaCastError>;
}
