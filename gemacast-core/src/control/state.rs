use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use axum::http::HeaderMap;
use tokio::sync::mpsc;

use crate::control::auth::{AuthorizedSession, SessionAuthorizer};
use crate::control::commands::ControlCommand;
use crate::control::types::{PresenceResponse, WsEvent};
use crate::domain::types::DeviceId;
use crate::ports::process_lister::ProcessLister;

/// Shared state provided to every control endpoint.
///
/// The state owns the dispatcher boundary, streamer identity, authorization
/// state, and the active WebSocket registry. Endpoint handlers only translate
/// HTTP requests into commands and read responses from these ports.
#[derive(Clone)]
pub struct ControlServerState<P: ProcessLister + 'static> {
    pub command_tx: mpsc::Sender<ControlCommand>,
    pub is_broadcasting: Arc<AtomicBool>,
    pub streamer_id: DeviceId,
    pub streamer_name: String,
    pub ws_connections: Arc<Mutex<HashMap<DeviceId, mpsc::Sender<WsEvent>>>>,
    pub process_lister: P,
    pub authorizer: SessionAuthorizer,
    pub pc_certificate_fingerprint: String,
}

impl<P: ProcessLister + 'static> ControlServerState<P> {
    pub(crate) fn build_presence(&self) -> PresenceResponse {
        PresenceResponse {
            device_id: self.streamer_id.clone(),
            streamer_name: self.streamer_name.clone(),
            is_offline: !self.is_broadcasting.load(Ordering::Relaxed),
            pc_network_link: None,
            device_registered: None,
            session_token: None,
            session_generation: None,
            pending_request_id: None,
            device_auth_challenge: None,
            pc_certificate_fingerprint: Some(self.pc_certificate_fingerprint.clone()),
            pc_output_volume: None,
        }
    }

    pub(crate) fn authenticate_device(
        &self,
        headers: &HeaderMap,
        device_id: &DeviceId,
    ) -> Option<AuthorizedSession> {
        bearer_token(headers).and_then(|token| self.authorizer.authenticate(device_id, token))
    }

    pub(crate) fn authenticate_token(&self, headers: &HeaderMap) -> Option<AuthorizedSession> {
        bearer_token(headers).and_then(|token| self.authorizer.authenticate_token(token))
    }
}

fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
}
