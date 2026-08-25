use async_trait::async_trait;
use gemacast_core::control::types::{ConnectReq, PresenceResponse};
use gemacast_core::domain::types::{AudioSource, DeviceId, ProcessInfo, StreamerCapabilities};
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

/// Sends control commands to a PC streamer over certificate-pinned HTTPS.
///
/// **Production**: [`crate::adapters::HttpStreamerControlClient`]
/// **Tests**: [`crate::testing::mocks::MockStreamerControlClient`]
#[async_trait]
pub trait StreamerControlClient: Send + Sync {
    /// Send a connect request to the streamer, returning the PC's presence response.
    async fn connect(&self, req: ConnectReq) -> Result<PresenceResponse, String>;

    /// Send a disconnect request to the streamer.
    async fn disconnect(&self, device_id: DeviceId) -> Result<(), String>;

    /// Request the list of available audio sources.
    async fn get_audio_sources(&self) -> Result<(Vec<AudioSource>, StreamerCapabilities), String>;

    /// Probe the streamer for its current state.
    async fn probe(&self, device_id: Option<DeviceId>) -> Result<PresenceResponse, String>;

    /// Request the streamer to change the audio source for a device.
    async fn change_source(&self, device_id: DeviceId, source: AudioSource) -> Result<(), String>;

    /// Request the streamer to change the encoding bitrate for a device.
    async fn change_bitrate(&self, device_id: DeviceId, bitrate: Option<i32>)
    -> Result<(), String>;

    /// Request the list of capturable processes from the streamer.
    async fn get_process_list(&self) -> Result<Vec<ProcessInfo>, String>;
}

/// Factory for creating [`StreamerControlClient`] instances, one per IP address.
///
/// **Production**: [`crate::adapters::HttpStreamerControlClientFactory`]
/// **Tests**: [`crate::testing::mocks::MockStreamerControlClientFactory`]
pub trait StreamerControlClientFactory: Send + Sync {
    fn create(&self, ip: IpAddr) -> Arc<dyn StreamerControlClient>;

    /// A client whose requests give up after `timeout`.
    ///
    /// Link recovery polls on a short interval and needs a request that fails
    /// inside that interval; the default client waits 10 s, which would make
    /// the poll period meaningless. Defaults to [`Self::create`] so mocks —
    /// which do no I/O and cannot time out — need not implement it.
    fn create_with_timeout(
        &self,
        ip: IpAddr,
        _timeout: Duration,
    ) -> Arc<dyn StreamerControlClient> {
        self.create(ip)
    }

    /// Return the current session token for WebSocket authentication.
    fn session_token(&self, _ip: IpAddr, _device_id: &DeviceId) -> Option<String> {
        None
    }

    /// Return the complete authenticated transport state for WSS pinning.
    fn session_credentials(
        &self,
        _ip: IpAddr,
        _device_id: &DeviceId,
    ) -> Option<gemacast_core::control::http_client::ControlCredentials> {
        None
    }
}
