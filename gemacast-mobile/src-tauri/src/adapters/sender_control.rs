use async_trait::async_trait;
use std::net::IpAddr;
use std::sync::Arc;

use gemacast_core::control::types::{ConnectReq, PresenceResponse};
use gemacast_core::domain::types::{AudioSource, DeviceId, ProcessInfo, SenderCapabilities};

use crate::traits::{SenderControlClient, SenderControlClientFactory};

/// Wraps `gemacast_core::control::HttpControlClient` behind the trait.
pub struct HttpSenderControlClient {
    client: gemacast_core::control::HttpControlClient,
}

impl HttpSenderControlClient {
    pub fn new(ip: IpAddr) -> Self {
        Self {
            client: gemacast_core::control::HttpControlClient::new(ip),
        }
    }
}

#[async_trait]
impl SenderControlClient for HttpSenderControlClient {
    async fn connect(&self, req: ConnectReq) -> Result<(), String> {
        self.client
            .send_connect_request(req)
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    async fn disconnect(&self, device_id: DeviceId) -> Result<(), String> {
        self.client
            .send_disconnect_request(device_id)
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    async fn get_audio_sources(&self) -> Result<(Vec<AudioSource>, SenderCapabilities), String> {
        self.client
            .request_audio_sources()
            .await
            .map_err(|e| e.to_string())
    }

    async fn probe(&self, device_id: Option<DeviceId>) -> Result<PresenceResponse, String> {
        self.client
            .send_probe(device_id)
            .await
            .map_err(|e| e.to_string())
    }

    async fn change_source(&self, device_id: DeviceId, source: AudioSource) -> Result<(), String> {
        self.client
            .send_change_source_request(device_id, source)
            .await
            .map_err(|e| e.to_string())
    }

    async fn change_bitrate(
        &self,
        device_id: DeviceId,
        bitrate: Option<i32>,
    ) -> Result<(), String> {
        self.client
            .send_change_bitrate_request(device_id, bitrate)
            .await
            .map_err(|e| e.to_string())
    }

    async fn get_process_list(&self) -> Result<Vec<ProcessInfo>, String> {
        self.client
            .request_process_list()
            .await
            .map_err(|e| e.to_string())
    }
}

/// Creates [`HttpSenderControlClient`] instances on demand.
pub struct HttpSenderControlClientFactory;

impl SenderControlClientFactory for HttpSenderControlClientFactory {
    fn create(&self, ip: IpAddr) -> Arc<dyn SenderControlClient> {
        Arc::new(HttpSenderControlClient::new(ip))
    }
}
