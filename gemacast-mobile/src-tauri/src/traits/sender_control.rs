use async_trait::async_trait;
use gemacast_core::control::types::{ConnectReq, PresenceResponse};
use gemacast_core::types::{AudioSource, DeviceId, ProcessInfo, SenderCapabilities};
use std::net::IpAddr;
use std::sync::Arc;

/// Sends control commands to a PC sender over HTTP.
///
/// **Production**: [`crate::adapters::HttpSenderControlClient`]
/// **Tests**: [`crate::testing::mocks::MockSenderControlClient`]
#[async_trait]
pub trait SenderControlClient: Send + Sync {
    /// Send a connect request to the sender.
    async fn connect(&self, req: ConnectReq) -> Result<(), String>;

    /// Send a disconnect request to the sender.
    async fn disconnect(&self, device_id: DeviceId) -> Result<(), String>;

    /// Request the list of available audio sources.
    async fn get_audio_sources(&self) -> Result<(Vec<AudioSource>, SenderCapabilities), String>;

    /// Probe the sender for its current state.
    async fn probe(&self, device_id: Option<DeviceId>) -> Result<PresenceResponse, String>;

    /// Request the sender to change the audio source for a device.
    async fn change_source(&self, device_id: DeviceId, source: AudioSource) -> Result<(), String>;

    /// Request the sender to change the encoding bitrate for a device.
    async fn change_bitrate(&self, device_id: DeviceId, bitrate: Option<i32>)
    -> Result<(), String>;

    /// Request the list of capturable processes from the sender.
    async fn get_process_list(&self) -> Result<Vec<ProcessInfo>, String>;
}

/// Factory for creating [`SenderControlClient`] instances, one per IP address.
///
/// **Production**: [`crate::adapters::HttpSenderControlClientFactory`]
/// **Tests**: [`crate::testing::mocks::MockSenderControlClientFactory`]
pub trait SenderControlClientFactory: Send + Sync {
    fn create(&self, ip: IpAddr) -> Arc<dyn SenderControlClient>;
}
