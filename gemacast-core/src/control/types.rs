use serde::{Deserialize, Serialize};

use crate::types::{AudioSource, ConnectionMode, DeviceId, JitterConfig};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectReq {
    pub device_id: DeviceId,
    pub device_name: String,
    pub source: AudioSource,
    pub mode: ConnectionMode,
    pub jitter_config: JitterConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DisconnectReq {
    pub device_id: DeviceId,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeReq {
    #[serde(default)]
    pub device_id: Option<DeviceId>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangeSourceReq {
    pub device_id: DeviceId,
    pub source: AudioSource,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourcesResponse {
    pub sources: Vec<AudioSource>,
    pub capabilities: crate::types::SenderCapabilities,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PresenceResponse {
    pub device_id: DeviceId,
    pub sender_name: String,
    pub is_offline: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessListResponse {
    pub processes: Vec<crate::types::ProcessInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE", tag = "type", content = "payload")]
pub enum WsEvent {
    Disconnect,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE", tag = "type", content = "payload")]
pub enum WsCommand {
    Disconnect
}