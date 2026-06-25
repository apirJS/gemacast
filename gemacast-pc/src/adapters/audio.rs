use crate::traits::AudioController;
use async_trait::async_trait;
use gemacast_core::domain::types::{AudioSource, DeviceId};
use gemacast_core::stream::sender::AudioStreamCommand;
use std::net::SocketAddr;
use tokio::sync::mpsc;

/// Sends [`AudioStreamCommand`]s to the audio engine via an `mpsc` channel.
pub struct ChannelAudioController {
    tx: mpsc::Sender<AudioStreamCommand>,
}

impl ChannelAudioController {
    pub fn new(tx: mpsc::Sender<AudioStreamCommand>) -> Self {
        Self { tx }
    }
}

#[async_trait]
impl AudioController for ChannelAudioController {
    async fn subscribe(
        &self,
        device_id: DeviceId,
        target_addr: Option<SocketAddr>,
        source: Option<AudioSource>,
        bitrate: Option<i32>,
    ) {
        let _ = self
            .tx
            .send(AudioStreamCommand::Subscribe {
                device_id,
                target_addr,
                source,
                bitrate,
            })
            .await;
    }

    async fn unsubscribe(&self, device_id: &DeviceId) {
        let _ = self
            .tx
            .send(AudioStreamCommand::Unsubscribe {
                device_id: device_id.clone(),
            })
            .await;
    }

    async fn change_source(&self, device_id: DeviceId, source: AudioSource) {
        let _ = self
            .tx
            .send(AudioStreamCommand::ChangeSource { device_id, source })
            .await;
    }

    async fn change_bitrate(&self, device_id: DeviceId, bitrate: Option<i32>) {
        let _ = self
            .tx
            .send(AudioStreamCommand::ChangeBitrate { device_id, bitrate })
            .await;
    }

    async fn shutdown(&self) {
        let _ = self.tx.send(AudioStreamCommand::Shutdown).await;
    }
}
