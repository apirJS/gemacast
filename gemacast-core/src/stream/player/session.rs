use crate::control::SessionGeneration;
use crate::domain::types::DeviceId;

/// Credentials that bind an audio transport to an authorized control session.
#[derive(Debug, Clone)]
pub struct AudioSessionCredentials {
    pub device_id: DeviceId,
    pub session_token: Option<String>,
    pub session_generation: Option<SessionGeneration>,
}
