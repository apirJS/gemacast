use serde::{Deserialize, Serialize};

use crate::types::{DeviceId, TransportType};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ControlMessage {
    Probe {
        #[serde(default)]
        device_id: Option<DeviceId>,
    },
    Presence {
        device_id: DeviceId,
        sender_name: String,
        #[serde(default)]
        is_offline: bool,
        #[serde(default)]
        transport: Option<TransportType>,
    },
    Disconnect {
        device_id: DeviceId,
    },
}
