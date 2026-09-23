use std::time::Duration;

use base64::Engine;

use crate::control::device_auth::{build_device_auth_transcript, pairing_code};
use crate::control::http_client::{DeviceAuthSigner, HttpControlClient};
use crate::control::http_transport::{build_https_client, request_error};
use crate::control::tls::response_certificate_fingerprint;
use crate::control::types::{ConnectReq, DeviceAuthRequest, PresenceResponse};
use crate::domain::error::{ControlError, GemaCastError};
use crate::domain::types::DeviceId;

impl HttpControlClient {
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
            let response = Self::authorize(client.post(format!("{}/connect", self.base_url)), None)
                .timeout(Self::CONNECT_TIMEOUT)
                .json(&connect_req)
                .send()
                .await
                .map_err(request_error)?;
            let observed_fingerprint = response_certificate_fingerprint(&response)
                .map_err(|reason| ControlError::Rejected { reason })?;
            let presence: PresenceResponse = Self::ensure_success(response)
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
                    reason: "streamer requested device authentication, but no signer is available"
                        .into(),
                })?;
                let auth = connect_req
                    .device_auth
                    .as_mut()
                    .ok_or_else(|| ControlError::Rejected {
                        reason: "streamer requested device authentication, but the request identity is missing"
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
                        reason: "streamer returned an invalid pairing comparison code".into(),
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
                let phone_confirmed = if trusted_fingerprint.is_none()
                    || challenge.requires_approval
                {
                    if !signer
                        .confirm_pc_identity(
                            &presence.device_id,
                            &presence.streamer_name,
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
                    true
                } else {
                    false
                };
                auth.challenge_id = Some(challenge.challenge_id.clone());
                auth.signature = Some(
                    signer
                        .sign(&transcript)
                        .map_err(|reason| ControlError::Rejected { reason })?,
                );
                auth.phone_confirmation = phone_confirmed.then_some(true);
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
                let new_credentials = super::http_client::ControlCredentials {
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
}
