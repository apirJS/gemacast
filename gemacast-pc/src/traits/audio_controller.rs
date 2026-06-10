use async_trait::async_trait;
use gemacast_core::types::{AudioSource, DeviceId};
use std::net::SocketAddr;

/// Sends commands to the audio stream engine.
///
/// **Production**: [`crate::adapters::ChannelAudioController`] wrapping `mpsc::Sender<AudioStreamCommand>`.
/// **Tests**: [`crate::testing::mocks::MockAudioController`] that records calls.
#[async_trait]
pub trait AudioController: Send + Sync {
    /// Start streaming audio to a device.
    ///
    /// `target_addr` is `None` for ADB/TCP devices (loopback), `Some` for UDP/WiFi.
    async fn subscribe(
        &self,
        device_id: DeviceId,
        target_addr: Option<SocketAddr>,
        source: Option<AudioSource>,
        bitrate: Option<i32>,
    );

    /// Stop streaming audio to a device.
    async fn unsubscribe(&self, device_id: &DeviceId);

    /// Switch the audio source for a device.
    async fn change_source(&self, device_id: DeviceId, source: AudioSource);

    /// Change the encoding bitrate for a device.
    async fn change_bitrate(&self, device_id: DeviceId, bitrate: Option<i32>);

    /// Shut down the audio engine entirely.
    async fn shutdown(&self);
}
