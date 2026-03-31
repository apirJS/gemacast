use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BroadcastPayload {
    pub device_id: String,
    pub device_name: String,
    #[serde(default)]
    pub is_offline: bool,
}

#[derive(Debug, Clone)]
pub struct DiscoveredDevice {
    pub device_id: String,
    pub device_name: String,
    pub addr: std::net::SocketAddr,
    pub last_seen: std::time::Instant,
    pub is_offline: bool,
}

impl DiscoveredDevice {
    pub fn from_broadcast(payload: BroadcastPayload, addr: std::net::SocketAddr) -> Self {
        Self {
            device_id: payload.device_id,
            device_name: payload.device_name,
            last_seen: std::time::Instant::now(),
            addr,
            is_offline: payload.is_offline,
        }
    }
}
