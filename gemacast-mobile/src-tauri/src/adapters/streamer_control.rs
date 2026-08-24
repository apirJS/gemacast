use async_trait::async_trait;
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gemacast_core::control::types::{ConnectReq, PresenceResponse};
use gemacast_core::domain::types::{AudioSource, DeviceId, ProcessInfo, StreamerCapabilities};

use crate::traits::{StreamerControlClient, StreamerControlClientFactory};

/// Wraps `gemacast_core::control::HttpControlClient` behind the trait.
pub struct HttpStreamerControlClient {
    client: gemacast_core::control::HttpControlClient,
    signer: Arc<dyn gemacast_core::control::http_client::DeviceAuthSigner>,
}

impl HttpStreamerControlClient {
    pub fn new(
        ip: IpAddr,
        credentials: Arc<Mutex<Option<gemacast_core::control::http_client::ControlCredentials>>>,
        signer: Arc<dyn gemacast_core::control::http_client::DeviceAuthSigner>,
    ) -> Self {
        Self {
            client: gemacast_core::control::HttpControlClient::with_shared_credentials(
                ip,
                Duration::from_secs(10),
                credentials,
            ),
            signer,
        }
    }

    pub fn with_timeout(
        ip: IpAddr,
        timeout: Duration,
        credentials: Arc<Mutex<Option<gemacast_core::control::http_client::ControlCredentials>>>,
        signer: Arc<dyn gemacast_core::control::http_client::DeviceAuthSigner>,
    ) -> Self {
        Self {
            client: gemacast_core::control::HttpControlClient::with_shared_credentials(
                ip,
                timeout,
                credentials,
            ),
            signer,
        }
    }
}

#[async_trait]
impl StreamerControlClient for HttpStreamerControlClient {
    async fn connect(&self, req: ConnectReq) -> Result<PresenceResponse, String> {
        self.client
            .send_connect_request_with_signer(req, Some(self.signer.as_ref()))
            .await
            .map_err(|e| e.to_string())
    }

    async fn disconnect(&self, device_id: DeviceId) -> Result<(), String> {
        self.client
            .send_disconnect_request(device_id)
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    async fn get_audio_sources(&self) -> Result<(Vec<AudioSource>, StreamerCapabilities), String> {
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

/// Creates [`HttpStreamerControlClient`] instances on demand.
pub struct HttpStreamerControlClientFactory {
    credentials: Mutex<
        HashMap<
            IpAddr,
            Arc<Mutex<Option<gemacast_core::control::http_client::ControlCredentials>>>,
        >,
    >,
    signer: Arc<dyn gemacast_core::control::http_client::DeviceAuthSigner>,
}

impl HttpStreamerControlClientFactory {
    pub fn new(signer: Arc<dyn gemacast_core::control::http_client::DeviceAuthSigner>) -> Self {
        Self {
            credentials: Mutex::new(HashMap::new()),
            signer,
        }
    }

    fn credentials(
        &self,
        ip: IpAddr,
    ) -> Arc<Mutex<Option<gemacast_core::control::http_client::ControlCredentials>>> {
        self.credentials
            .lock()
            .map(|mut credentials| {
                credentials
                    .entry(ip)
                    .or_insert_with(|| Arc::new(Mutex::new(None)))
                    .clone()
            })
            .unwrap_or_else(|_| Arc::new(Mutex::new(None)))
    }
}

impl StreamerControlClientFactory for HttpStreamerControlClientFactory {
    fn create(&self, ip: IpAddr) -> Arc<dyn StreamerControlClient> {
        Arc::new(HttpStreamerControlClient::new(
            ip,
            self.credentials(ip),
            self.signer.clone(),
        ))
    }

    fn create_with_timeout(&self, ip: IpAddr, timeout: Duration) -> Arc<dyn StreamerControlClient> {
        Arc::new(HttpStreamerControlClient::with_timeout(
            ip,
            timeout,
            self.credentials(ip),
            self.signer.clone(),
        ))
    }

    fn session_token(&self, ip: IpAddr, device_id: &DeviceId) -> Option<String> {
        self.credentials
            .lock()
            .ok()
            .and_then(|credentials| credentials.get(&ip).cloned())
            .and_then(|credentials| {
                credentials.lock().ok().and_then(|credentials| {
                    credentials
                        .as_ref()
                        .filter(|credentials| &credentials.device_id == device_id)
                        .map(|credentials| credentials.token.clone())
                })
            })
    }

    fn session_credentials(
        &self,
        ip: IpAddr,
        device_id: &DeviceId,
    ) -> Option<gemacast_core::control::http_client::ControlCredentials> {
        self.credentials
            .lock()
            .ok()
            .and_then(|credentials| credentials.get(&ip).cloned())
            .and_then(|credentials| {
                credentials.lock().ok().and_then(|credentials| {
                    credentials
                        .as_ref()
                        .filter(|credentials| &credentials.device_id == device_id)
                        .cloned()
                })
            })
    }
}
