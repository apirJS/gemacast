use std::net::IpAddr;
use std::time::Duration;

use crate::control::types::{
    ChangeSourceReq, ConnectReq, DisconnectReq, PresenceResponse, ProcessListResponse, ProbeReq,
    SourcesResponse,
};
use crate::error::{ControlError, GemaCastError};
use crate::network::Ports;
use crate::types::{AudioSource, DeviceId, ProcessInfo, SenderCapabilities};

pub struct HttpControlClient {
    client: reqwest::Client,
    base_url: String,
}

impl HttpControlClient {
    pub fn new(target_ip: IpAddr) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(3))
            .build()
            .unwrap_or_default();

        let base_url = format!("http://{}:{}", target_ip, Ports::CONTROL);
        Self { client, base_url }
    }

    pub async fn send_connect_request(
        &self,
        connect_req: ConnectReq,
    ) -> Result<PresenceResponse, GemaCastError> {
        let resp = self
            .client
            .post(format!("{}/connect", self.base_url))
            .json(&connect_req)
            .send()
            .await
            .map_err(|e| ControlError::HttpRequestFailed(e.to_string()))?;

        if resp.status().is_client_error() {
            let presence: PresenceResponse = resp
                .json()
                .await
                .map_err(|e| ControlError::HttpRequestFailed(e.to_string()))?;
            return Err(ControlError::Rejected {
                reason: format!("sender {} is offline", presence.sender_name),
            }
            .into());
        }

        resp.json()
            .await
            .map_err(|e| ControlError::HttpRequestFailed(e.to_string()).into())
    }

    pub async fn send_disconnect_request(&self, device_id: DeviceId) -> Result<(), GemaCastError> {
        self.client
            .post(format!("{}/disconnect", self.base_url))
            .json(&DisconnectReq { device_id })
            .send()
            .await
            .map_err(|e| ControlError::HttpRequestFailed(e.to_string()))?;
        Ok(())
    }

    pub async fn request_audio_sources(
        &self,
    ) -> Result<(Vec<AudioSource>, SenderCapabilities), GemaCastError> {
        let resp: SourcesResponse = self
            .client
            .get(format!("{}/sources", self.base_url))
            .send()
            .await
            .map_err(|e| ControlError::HttpRequestFailed(e.to_string()))?
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
        self.client
            .post(format!("{}/change-source", self.base_url))
            .json(&ChangeSourceReq { device_id, source })
            .send()
            .await
            .map_err(|e| ControlError::HttpRequestFailed(e.to_string()))?;
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

        resp.json()
            .await
            .map_err(|e| ControlError::HttpRequestFailed(e.to_string()).into())
    }

    pub async fn request_process_list(&self) -> Result<Vec<ProcessInfo>, GemaCastError> {
        let resp: ProcessListResponse = self
            .client
            .get(format!("{}/processes", self.base_url))
            .send()
            .await
            .map_err(|e| ControlError::HttpRequestFailed(e.to_string()))?
            .json()
            .await
            .map_err(|e| ControlError::HttpRequestFailed(e.to_string()))?;

        Ok(resp.processes)
    }
}