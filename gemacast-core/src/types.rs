use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ControlMessage {
    Presence {
        sender_id: String,
        sender_name: String,
        #[serde(default)]
        is_offline: bool,
        #[serde(default)]
        volume: Option<f32>,
        #[serde(default)]
        is_muted: Option<bool>,
    },
    Connect {
        device_id: String,
        device_name: String,
    },
    Disconnect {
        device_id: String,
    },
    SetSystemVolume {
        device_id: String,
        level: f32,
    },
    SetSystemMute {
        device_id: String,
        muted: bool,
    },
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredDevice {
    pub device_id: String,
    pub device_name: String,
    pub addr: std::net::SocketAddr,
    #[serde(skip)]
    pub last_seen: std::time::Instant,
    pub is_offline: bool,
    pub volume: Option<f32>,
    pub is_muted: Option<bool>,
}

impl DiscoveredDevice {
    pub fn from_presence(
        sender_id: String,
        sender_name: String,
        is_offline: bool,
        addr: std::net::SocketAddr,
        volume: Option<f32>,
        is_muted: Option<bool>,
    ) -> Self {
        Self {
            device_id: sender_id,
            device_name: sender_name,
            last_seen: std::time::Instant::now(),
            addr,
            is_offline,
            volume,
            is_muted,
        }
    }
}
