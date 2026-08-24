use serde::{Deserialize, Serialize};

use crate::domain::types::{DeviceId, TransportType};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ControlMessage {
    Probe {
        #[serde(default)]
        device_id: Option<DeviceId>,
    },
    Presence {
        device_id: DeviceId,
        streamer_name: String,
        #[serde(default)]
        is_offline: bool,
        #[serde(default)]
        transport: Option<TransportType>,
    },
    Disconnect {
        device_id: DeviceId,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presence_should_round_trip_through_json() {
        let msg = ControlMessage::Presence {
            device_id: DeviceId("PC_TEST".to_string()),
            streamer_name: "My PC".to_string(),
            is_offline: false,
            transport: None,
        };
        let json = serde_json::to_string(&msg).unwrap();

        // The enum's `rename_all = "camelCase"` renames *variants*, not variant
        // fields, so this ships snake_case. Pinned here because the key is a
        // wire contract with the phone and destructuring below cannot see it.
        assert!(
            json.contains("\"streamer_name\""),
            "presence must ship streamer_name on the wire, got {json}"
        );

        let parsed: ControlMessage = serde_json::from_str(&json).unwrap();

        match parsed {
            ControlMessage::Presence {
                device_id,
                streamer_name,
                is_offline,
                transport,
            } => {
                assert_eq!(device_id.0, "PC_TEST");
                assert_eq!(streamer_name, "My PC");
                assert!(!is_offline);
                assert!(transport.is_none());
            }
            _ => panic!("Expected ControlMessage::Presence"),
        }
    }

    #[test]
    fn disconnect_should_round_trip_through_json() {
        let msg = ControlMessage::Disconnect {
            device_id: DeviceId("phone_123".to_string()),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: ControlMessage = serde_json::from_str(&json).unwrap();

        match parsed {
            ControlMessage::Disconnect { device_id } => {
                assert_eq!(device_id.0, "phone_123");
            }
            _ => panic!("Expected ControlMessage::Disconnect"),
        }
    }

    #[test]
    fn probe_should_round_trip_through_json() {
        let msg = ControlMessage::Probe {
            device_id: Some(DeviceId("dev_42".to_string())),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: ControlMessage = serde_json::from_str(&json).unwrap();

        match parsed {
            ControlMessage::Probe { device_id } => {
                assert_eq!(device_id.unwrap().0, "dev_42");
            }
            _ => panic!("Expected ControlMessage::Probe"),
        }
    }
}
