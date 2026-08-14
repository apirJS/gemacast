use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::control::types::{
    ConnectReq, ControlErrorResponse, DisconnectReq, PresenceResponse, ProbeReq,
    ProcessListResponse, SourcesResponse,
};
use crate::domain::error::{ControlError, GemaCastError};
use crate::domain::types::{AudioSource, DeviceId, ProcessInfo, SenderCapabilities};
use crate::network::Ports;

pub struct HttpControlClient {
    client: reqwest::Client,
    base_url: String,
    credentials: Arc<Mutex<Option<ControlCredentials>>>,
}

#[derive(Debug, Clone)]
pub struct ControlCredentials {
    pub device_id: DeviceId,
    pub token: String,
    pub generation: crate::control::SessionGeneration,
}

impl HttpControlClient {
    /// Default request timeout, generous enough for `/connect` to complete a
    /// full capture-and-encode start-up on the PC side.
    const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);
    const CONNECT_TIMEOUT: Duration = Duration::from_secs(70);

    pub fn new(target_ip: IpAddr) -> Self {
        Self::with_timeout(target_ip, Self::DEFAULT_TIMEOUT)
    }

    /// Same client with an explicit request timeout.
    ///
    /// Link recovery polls `/probe` every 2 s to find out whether the PC came
    /// back; against [`Self::DEFAULT_TIMEOUT`] a single unanswered request
    /// would span five poll intervals, so the poll period would be a fiction
    /// and the 60 s recovery budget would buy six attempts instead of thirty.
    /// The caller sets a timeout no longer than its own interval.
    pub fn with_timeout(target_ip: IpAddr, timeout: Duration) -> Self {
        Self::with_shared_credentials(target_ip, timeout, Arc::new(Mutex::new(None)))
    }

    pub fn with_shared_credentials(
        target_ip: IpAddr,
        timeout: Duration,
        credentials: Arc<Mutex<Option<ControlCredentials>>>,
    ) -> Self {
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .unwrap_or_default();

        let base_url = format!("http://{}:{}", target_ip, Ports::CONTROL);
        Self {
            client,
            base_url,
            credentials,
        }
    }

    fn authorize(
        &self,
        request: reqwest::RequestBuilder,
        device_id: Option<&DeviceId>,
    ) -> reqwest::RequestBuilder {
        let credential = self
            .credentials
            .lock()
            .ok()
            .and_then(|credentials| credentials.clone());
        match credential {
            Some(credential)
                if device_id.is_none_or(|device_id| device_id == &credential.device_id) =>
            {
                request.bearer_auth(credential.token)
            }
            _ => request,
        }
    }

    pub fn session_token(&self, device_id: &DeviceId) -> Option<String> {
        self.credentials.lock().ok().and_then(|credentials| {
            credentials
                .as_ref()
                .filter(|credentials| &credentials.device_id == device_id)
                .map(|credentials| credentials.token.clone())
        })
    }

    async fn ensure_success(
        response: reqwest::Response,
    ) -> Result<reqwest::Response, GemaCastError> {
        if response.status().is_success() {
            return Ok(response);
        }

        let status = response.status();
        let reason = response
            .json::<ControlErrorResponse>()
            .await
            .map(|error| format!("{} ({})", error.message, error.code))
            .unwrap_or_else(|_| format!("HTTP {status}"));
        Err(ControlError::Rejected { reason }.into())
    }

    pub async fn send_connect_request(
        &self,
        mut connect_req: ConnectReq,
    ) -> Result<PresenceResponse, GemaCastError> {
        let device_id = connect_req.device_id.clone();
        let deadline = tokio::time::Instant::now() + Self::CONNECT_TIMEOUT;
        loop {
            let resp = self
                .authorize(
                    self.client.post(format!("{}/connect", self.base_url)),
                    Some(&device_id),
                )
                .timeout(Self::DEFAULT_TIMEOUT)
                .json(&connect_req)
                .send()
                .await
                .map_err(|e| ControlError::HttpRequestFailed(e.to_string()))?;

            let presence: PresenceResponse = Self::ensure_success(resp)
                .await?
                .json()
                .await
                .map_err(|e| ControlError::HttpRequestFailed(e.to_string()))?;
            if let (Some(token), Some(generation)) =
                (presence.session_token.clone(), presence.session_generation)
            {
                if let Ok(mut credentials) = self.credentials.lock() {
                    *credentials = Some(ControlCredentials {
                        device_id,
                        token,
                        generation,
                    });
                }
                return Ok(presence);
            }

            let Some(request_id) = presence.pending_request_id.clone() else {
                return Ok(presence);
            };
            if tokio::time::Instant::now() >= deadline {
                return Err(ControlError::Rejected {
                    reason: format!("connection approval {request_id} timed out"),
                }
                .into());
            }
            connect_req.pending_request_id = Some(request_id);
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }

    pub async fn send_disconnect_request(&self, device_id: DeviceId) -> Result<(), GemaCastError> {
        let response = self
            .authorize(
                self.client.post(format!("{}/disconnect", self.base_url)),
                Some(&device_id),
            )
            .json(&DisconnectReq { device_id })
            .send()
            .await
            .map_err(|e| ControlError::HttpRequestFailed(e.to_string()))?;
        Self::ensure_success(response).await?;
        if let Ok(mut credentials) = self.credentials.lock() {
            *credentials = None;
        }
        Ok(())
    }

    pub async fn request_audio_sources(
        &self,
    ) -> Result<(Vec<AudioSource>, SenderCapabilities), GemaCastError> {
        let response = self
            .authorize(self.client.get(format!("{}/sources", self.base_url)), None)
            .send()
            .await
            .map_err(|e| ControlError::HttpRequestFailed(e.to_string()))?;
        let resp: SourcesResponse = Self::ensure_success(response)
            .await?
            .json()
            .await
            .map_err(|e| ControlError::HttpRequestFailed(e.to_string()))?;

        Ok((resp.sources, resp.capabilities))
    }

    pub async fn send_change_source_request(
        &self,
        device_id: DeviceId,
        source: AudioSource,
    ) -> Result<(), GemaCastError> {
        let response = self
            .authorize(
                self.client.post(format!("{}/change-source", self.base_url)),
                Some(&device_id),
            )
            .json(&super::types::ChangeSourceReq { device_id, source })
            .send()
            .await
            .map_err(|e| ControlError::HttpRequestFailed(e.to_string()))?;
        Self::ensure_success(response).await?;
        Ok(())
    }

    pub async fn send_change_bitrate_request(
        &self,
        device_id: DeviceId,
        bitrate: Option<i32>,
    ) -> Result<(), GemaCastError> {
        let response = self
            .authorize(
                self.client
                    .post(format!("{}/change-bitrate", self.base_url)),
                Some(&device_id),
            )
            .json(&super::types::ChangeBitrateReq { device_id, bitrate })
            .send()
            .await
            .map_err(|e| ControlError::HttpRequestFailed(e.to_string()))?;
        Self::ensure_success(response).await?;
        Ok(())
    }

    pub async fn send_probe(
        &self,
        device_id: Option<DeviceId>,
    ) -> Result<PresenceResponse, GemaCastError> {
        let resp = self
            .client
            .post(format!("{}/probe", self.base_url))
            .json(&ProbeReq { device_id })
            .send()
            .await
            .map_err(|e| ControlError::HttpRequestFailed(e.to_string()))?;

        Self::ensure_success(resp)
            .await?
            .json()
            .await
            .map_err(|e| ControlError::HttpRequestFailed(e.to_string()).into())
    }

    pub async fn request_process_list(&self) -> Result<Vec<ProcessInfo>, GemaCastError> {
        let response = self
            .authorize(
                self.client.get(format!("{}/processes", self.base_url)),
                None,
            )
            .send()
            .await
            .map_err(|e| ControlError::HttpRequestFailed(e.to_string()))?;
        let resp: ProcessListResponse = Self::ensure_success(response)
            .await?
            .json()
            .await
            .map_err(|e| ControlError::HttpRequestFailed(e.to_string()))?;

        Ok(resp.processes)
    }
}
