use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;

use crate::control::device_auth::{build_device_auth_transcript, pairing_code};
use crate::control::tls::{client_config, response_certificate_fingerprint};
use crate::control::types::{
    ConnectReq, ControlErrorResponse, DeviceAuthRequest, DisconnectReq, PresenceResponse, ProbeReq,
    ProcessListResponse, SourcesResponse,
};
use crate::domain::error::{ControlError, GemaCastError};
use crate::domain::types::{AudioSource, DeviceId, ProcessInfo, SenderCapabilities};
use crate::network::Ports;

fn format_error_chain(error: &(dyn std::error::Error + 'static)) -> String {
    let mut message = error.to_string();
    let mut source = error.source();
    while let Some(error) = source {
        let detail = error.to_string();
        if !detail.is_empty() && !message.contains(&detail) {
            message.push_str(": ");
            message.push_str(&detail);
        }
        source = error.source();
    }
    message
}

fn request_error(error: reqwest::Error) -> GemaCastError {
    ControlError::HttpRequestFailed(format_error_chain(&error)).into()
}

pub struct HttpControlClient {
    bootstrap_client: reqwest::Client,
    base_url: String,
    request_timeout: Duration,
    credentials: Arc<Mutex<Option<ControlCredentials>>>,
}

#[derive(Debug, Clone)]
pub struct ControlCredentials {
    pub device_id: DeviceId,
    pub token: String,
    pub generation: crate::control::SessionGeneration,
    pub pc_device_id: DeviceId,
    pub pc_certificate_fingerprint: String,
}

/// Long-term receiver identity and persistent PC certificate pin storage.
///
/// Android keeps the private signing key in Android Keystore and stores only
/// approved PC certificate fingerprints in app-private preferences.
pub trait DeviceAuthSigner: Send + Sync {
    fn public_key(&self) -> Result<String, String>;
    fn sign(&self, transcript: &[u8]) -> Result<String, String>;

    fn trusted_pc_fingerprint(&self, _pc_id: &DeviceId) -> Result<Option<String>, String> {
        Ok(None)
    }

    fn confirm_pc_identity(
        &self,
        pc_id: &DeviceId,
        pc_name: &str,
        fingerprint: &str,
        pairing_code: &str,
        requires_approval: bool,
    ) -> Result<bool, String>;

    fn remember_pc_identity(&self, _pc_id: &DeviceId, _fingerprint: &str) -> Result<(), String> {
        Ok(())
    }
}

impl HttpControlClient {
    const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);
    const CONNECT_TIMEOUT: Duration = Duration::from_secs(70);

    pub fn new(target_ip: IpAddr) -> Self {
        Self::with_timeout(target_ip, Self::DEFAULT_TIMEOUT)
    }

    pub fn with_timeout(target_ip: IpAddr, timeout: Duration) -> Self {
        Self::with_shared_credentials(target_ip, timeout, Arc::new(Mutex::new(None)))
    }

    pub fn with_shared_credentials(
        target_ip: IpAddr,
        timeout: Duration,
        credentials: Arc<Mutex<Option<ControlCredentials>>>,
    ) -> Self {
        let bootstrap_client = build_https_client(timeout, None).unwrap_or_else(|reason| {
            panic!("failed to initialize the mandatory HTTPS control client: {reason}")
        });
        let base_url = format!("https://{}:{}", target_ip, Ports::CONTROL);
        Self {
            bootstrap_client,
            base_url,
            request_timeout: timeout,
            credentials,
        }
    }

    fn credential_for(&self, device_id: Option<&DeviceId>) -> Option<ControlCredentials> {
        self.credentials
            .lock()
            .ok()
            .and_then(|credentials| credentials.clone())
            .filter(|credential| {
                device_id.is_none_or(|device_id| device_id == &credential.device_id)
            })
    }

    fn client_for(
        &self,
        credential: Option<&ControlCredentials>,
    ) -> Result<reqwest::Client, GemaCastError> {
        credential.map_or_else(
            || Ok(self.bootstrap_client.clone()),
            |credential| {
                build_https_client(
                    self.request_timeout,
                    Some(&credential.pc_certificate_fingerprint),
                )
                .map_err(|reason| ControlError::Rejected { reason }.into())
            },
        )
    }

    fn authorize(
        request: reqwest::RequestBuilder,
        credential: Option<&ControlCredentials>,
    ) -> reqwest::RequestBuilder {
        match credential {
            Some(credential) => request.bearer_auth(&credential.token),
            None => request,
        }
    }

    pub fn session_token(&self, device_id: &DeviceId) -> Option<String> {
        self.credential_for(Some(device_id))
            .map(|credentials| credentials.token)
    }

    pub fn session_credentials(&self, device_id: &DeviceId) -> Option<ControlCredentials> {
        self.credential_for(Some(device_id))
    }

    async fn ensure_success(
        response: reqwest::Response,
    ) -> Result<reqwest::Response, GemaCastError> {
        if response.status().is_success() {
            return Ok(response);
        }

        let status = response.status();
        let error = response
            .json::<ControlErrorResponse>()
            .await
            .unwrap_or_else(|_| ControlErrorResponse {
                code: "http_error".into(),
                message: format!("HTTP {status}"),
            });
        Err(ControlError::RemoteRejected {
            code: error.code,
            reason: error.message,
        }
        .into())
    }

    fn verify_presence_certificate(
        presence: &PresenceResponse,
        observed_fingerprint: &str,
    ) -> Result<String, GemaCastError> {
        let advertised = presence
            .pc_certificate_fingerprint
            .as_deref()
            .ok_or_else(|| ControlError::Rejected {
                reason: "sender did not provide its PC certificate fingerprint".into(),
            })?;
        if !advertised.eq_ignore_ascii_case(observed_fingerprint) {
            return Err(ControlError::Rejected {
                reason: "sender identity does not match the HTTPS certificate".into(),
            }
            .into());
        }
        Ok(advertised.to_ascii_lowercase())
    }

    pub async fn send_connect_request(
        &self,
        connect_req: ConnectReq,
    ) -> Result<PresenceResponse, GemaCastError> {
        self.send_connect_request_with_signer(connect_req, None)
            .await
    }

    pub async fn send_connect_request_with_signer(
        &self,
        mut connect_req: ConnectReq,
        signer: Option<&dyn DeviceAuthSigner>,
    ) -> Result<PresenceResponse, GemaCastError> {
        let device_id = connect_req.device_id.clone();
        if connect_req.device_auth.is_none() {
            let signer = signer.ok_or_else(|| ControlError::Rejected {
                reason: "device authentication is unavailable".into(),
            })?;
            let mut nonce = [0u8; 32];
            getrandom::fill(&mut nonce).map_err(|error| ControlError::Rejected {
                reason: format!("failed to generate device nonce: {error}"),
            })?;
            connect_req.device_auth = Some(DeviceAuthRequest {
                public_key: signer
                    .public_key()
                    .map_err(|reason| ControlError::Rejected { reason })?,
                phone_nonce: base64::engine::general_purpose::STANDARD.encode(nonce),
                challenge_id: None,
                signature: None,
                phone_confirmation: None,
            });
        }

        let mut approval_deadline: Option<tokio::time::Instant> = None;
        // A connect always bootstraps against the certificate actually served
        // at this address. Reusing an IP-keyed certificate pin here can strand
        // the client when DHCP assigns the address to a different PC.
        let mut client = self.bootstrap_client.clone();
        let mut candidate_pc_identity: Option<(DeviceId, String)> = None;

        loop {
            let resp = Self::authorize(client.post(format!("{}/connect", self.base_url)), None)
                .timeout(Self::CONNECT_TIMEOUT)
                .json(&connect_req)
                .send()
                .await
                .map_err(request_error)?;
            let observed_fingerprint = response_certificate_fingerprint(&resp)
                .map_err(|reason| ControlError::Rejected { reason })?;
            let presence: PresenceResponse = Self::ensure_success(resp)
                .await?
                .json()
                .await
                .map_err(request_error)?;
            let pc_fingerprint =
                Self::verify_presence_certificate(&presence, &observed_fingerprint)?;

            if let Some(challenge) = presence.device_auth_challenge.as_ref() {
                if !challenge
                    .pc_certificate_fingerprint
                    .eq_ignore_ascii_case(&pc_fingerprint)
                {
                    return Err(ControlError::Rejected {
                        reason: "authentication challenge is bound to a different PC certificate"
                            .into(),
                    }
                    .into());
                }
                let signer = signer.ok_or_else(|| ControlError::Rejected {
                    reason: "sender requested device authentication, but no signer is available"
                        .into(),
                })?;
                let auth =
                    connect_req
                        .device_auth
                        .as_mut()
                        .ok_or_else(|| ControlError::Rejected {
                            reason: "sender requested device authentication, but the request identity is missing"
                                .into(),
                        })?;
                let transcript = build_device_auth_transcript(
                    &connect_req.device_id,
                    &presence.device_id,
                    &pc_fingerprint,
                    &auth.public_key,
                    &auth.phone_nonce,
                    &challenge.challenge_id,
                    &challenge.challenge,
                );
                let expected_code = pairing_code(&transcript);
                if challenge.pairing_code != expected_code {
                    return Err(ControlError::Rejected {
                        reason: "sender returned an invalid pairing comparison code".into(),
                    }
                    .into());
                }
                let trusted_fingerprint = signer
                    .trusted_pc_fingerprint(&presence.device_id)
                    .map_err(|reason| ControlError::Rejected { reason })?;
                if let Some(trusted_fingerprint) = trusted_fingerprint.as_deref()
                    && !trusted_fingerprint.eq_ignore_ascii_case(&pc_fingerprint)
                {
                    return Err(ControlError::Rejected {
                        reason:
                            "the paired PC certificate changed; forget this PC before pairing again"
                                .into(),
                    }
                    .into());
                }
                if (trusted_fingerprint.is_none() || challenge.requires_approval)
                    && !signer
                        .confirm_pc_identity(
                            &presence.device_id,
                            &presence.sender_name,
                            &pc_fingerprint,
                            &expected_code,
                            challenge.requires_approval,
                        )
                        .map_err(|reason| ControlError::Rejected { reason })?
                {
                    return Err(ControlError::Rejected {
                        reason: "PC identity confirmation was cancelled on the phone".into(),
                    }
                    .into());
                }
                auth.challenge_id = Some(challenge.challenge_id.clone());
                auth.signature = Some(
                    signer
                        .sign(&transcript)
                        .map_err(|reason| ControlError::Rejected { reason })?,
                );
                auth.phone_confirmation = Some(true);
                if trusted_fingerprint.is_none() {
                    candidate_pc_identity =
                        Some((presence.device_id.clone(), pc_fingerprint.clone()));
                }
                client = build_https_client(self.request_timeout, Some(&pc_fingerprint))
                    .map_err(|reason| ControlError::Rejected { reason })?;
                continue;
            }

            if let (Some(token), Some(generation)) =
                (presence.session_token.clone(), presence.session_generation)
            {
                let new_credentials = ControlCredentials {
                    device_id: device_id.clone(),
                    token,
                    generation,
                    pc_device_id: presence.device_id.clone(),
                    pc_certificate_fingerprint: pc_fingerprint,
                };
                if let Ok(mut credentials) = self.credentials.lock() {
                    *credentials = Some(new_credentials);
                }
                if let Some((pc_id, fingerprint)) = candidate_pc_identity.take()
                    && let Some(signer) = signer
                    && let Err(reason) = signer.remember_pc_identity(&pc_id, &fingerprint)
                {
                    let _ = self.send_disconnect_request(device_id.clone()).await;
                    return Err(ControlError::Rejected {
                        reason: format!("failed to remember the approved PC: {reason}"),
                    }
                    .into());
                }
                return Ok(presence);
            }

            let Some(request_id) = presence.pending_request_id.clone() else {
                return Ok(presence);
            };
            let deadline = *approval_deadline
                .get_or_insert_with(|| tokio::time::Instant::now() + Self::CONNECT_TIMEOUT);
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
        let credential = self.credential_for(Some(&device_id));
        let client = self.client_for(credential.as_ref())?;
        let response = Self::authorize(
            client.post(format!("{}/disconnect", self.base_url)),
            credential.as_ref(),
        )
        .json(&DisconnectReq { device_id })
        .send()
        .await
        .map_err(request_error)?;
        Self::ensure_success(response).await?;
        if let Ok(mut credentials) = self.credentials.lock() {
            *credentials = None;
        }
        Ok(())
    }

    pub async fn request_audio_sources(
        &self,
    ) -> Result<(Vec<AudioSource>, SenderCapabilities), GemaCastError> {
        let credential = self.credential_for(None);
        let client = self.client_for(credential.as_ref())?;
        let response = Self::authorize(
            client.get(format!("{}/sources", self.base_url)),
            credential.as_ref(),
        )
        .send()
        .await
        .map_err(request_error)?;
        let resp: SourcesResponse = Self::ensure_success(response)
            .await?
            .json()
            .await
            .map_err(request_error)?;
        Ok((resp.sources, resp.capabilities))
    }

    pub async fn send_change_source_request(
        &self,
        device_id: DeviceId,
        source: AudioSource,
    ) -> Result<(), GemaCastError> {
        let credential = self.credential_for(Some(&device_id));
        let client = self.client_for(credential.as_ref())?;
        let response = Self::authorize(
            client.post(format!("{}/change-source", self.base_url)),
            credential.as_ref(),
        )
        .json(&super::types::ChangeSourceReq { device_id, source })
        .send()
        .await
        .map_err(request_error)?;
        Self::ensure_success(response).await?;
        Ok(())
    }

    pub async fn send_change_bitrate_request(
        &self,
        device_id: DeviceId,
        bitrate: Option<i32>,
    ) -> Result<(), GemaCastError> {
        let credential = self.credential_for(Some(&device_id));
        let client = self.client_for(credential.as_ref())?;
        let response = Self::authorize(
            client.post(format!("{}/change-bitrate", self.base_url)),
            credential.as_ref(),
        )
        .json(&super::types::ChangeBitrateReq { device_id, bitrate })
        .send()
        .await
        .map_err(request_error)?;
        Self::ensure_success(response).await?;
        Ok(())
    }

    pub async fn send_probe(
        &self,
        device_id: Option<DeviceId>,
    ) -> Result<PresenceResponse, GemaCastError> {
        let credential = self.credential_for(device_id.as_ref());
        let client = self.client_for(credential.as_ref())?;
        let resp = client
            .post(format!("{}/probe", self.base_url))
            .json(&ProbeReq { device_id })
            .send()
            .await
            .map_err(request_error)?;
        let observed_fingerprint = response_certificate_fingerprint(&resp)
            .map_err(|reason| ControlError::Rejected { reason })?;
        let presence = Self::ensure_success(resp)
            .await?
            .json::<PresenceResponse>()
            .await
            .map_err(request_error)?;
        Self::verify_presence_certificate(&presence, &observed_fingerprint)?;
        Ok(presence)
    }

    pub async fn request_process_list(&self) -> Result<Vec<ProcessInfo>, GemaCastError> {
        let credential = self.credential_for(None);
        let client = self.client_for(credential.as_ref())?;
        let response = Self::authorize(
            client.get(format!("{}/processes", self.base_url)),
            credential.as_ref(),
        )
        .send()
        .await
        .map_err(request_error)?;
        let resp: ProcessListResponse = Self::ensure_success(response)
            .await?
            .json()
            .await
            .map_err(request_error)?;
        Ok(resp.processes)
    }
}

fn build_https_client(
    timeout: Duration,
    expected_fingerprint: Option<&str>,
) -> Result<reqwest::Client, String> {
    let tls_config = client_config(expected_fingerprint)?;
    reqwest::Client::builder()
        .timeout(timeout)
        // The control endpoint is a direct peer on the local network. Never
        // route it through HTTP(S)_PROXY or an Android system proxy: that can
        // turn a reachable private IP into a silent ten-second timeout before
        // the PC's listener sees any TCP connection.
        .no_proxy()
        .tls_info(true)
        .https_only(true)
        .pool_max_idle_per_host(0)
        .use_preconfigured_tls(tls_config)
        .build()
        .map_err(|error| format!("failed to configure HTTPS control client: {error}"))
}
