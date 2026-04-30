use serde::{Deserialize, Serialize};

use crate::types::{
    AudioSource, ConnectionMode, DeviceId, JitterConfig, SenderCapabilities, SenderId,
    TransportType,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ControlMessage {
    Probe {
        #[serde(default)]
        device_id: Option<DeviceId>,
    },
    Presence {
        sender_id: SenderId,
        sender_name: String,
        #[serde(default)]
        is_offline: bool,
        #[serde(default)]
        transport: Option<TransportType>,
    },
    Connect {
        device_id: DeviceId,
        device_name: String,
        #[serde(default)]
        mode: ConnectionMode,
        #[serde(default)]
        exclusive_mode: bool,
        #[serde(default)]
        jitter_config: JitterConfig,
        #[serde(default)]
        transport: Option<TransportType>,
        #[serde(default)]
        source: AudioSource,
    },
    Disconnect {
        device_id: DeviceId,
    },
    /// Request from receiver to sender: "What sources can I listen to?"
    GetSources {
        device_id: DeviceId,
    },
    /// Response from sender to receiver: available audio sources.
    SourceList {
        sources: Vec<AudioSource>,
        capabilities: SenderCapabilities,
    },
    /// Request from receiver: change my audio source while already connected.
    ChangeSource {
        device_id: DeviceId,
        source: AudioSource,
    },
}
