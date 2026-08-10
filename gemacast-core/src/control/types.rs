use serde::{Deserialize, Serialize};

use crate::domain::types::{AudioSource, ConnectionMode, DeviceId, JitterConfig, NetworkLink};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectReq {
    pub device_id: DeviceId,
    pub device_name: String,
    pub mode: ConnectionMode,
    pub jitter_config: JitterConfig,
    /// Desired bitrate in bits/sec. `None` = uncompressed raw PCM.
    #[serde(default = "default_bitrate")]
    pub bitrate: Option<i32>,

    #[serde(default)]
    pub source: Option<AudioSource>,

    /// The phone's detected network link type (e.g., WiFi 5 GHz, USB tether).
    /// Used by the PC to include in its response, and by the phone to compute
    /// the effective link pair for Auto preset selection.
    #[serde(default)]
    pub network_link: Option<NetworkLink>,
}

fn default_bitrate() -> Option<i32> {
    Some(128_000)
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
pub struct ChangeBitrateReq {
    pub device_id: DeviceId,
    pub bitrate: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourcesResponse {
    pub sources: Vec<AudioSource>,
    pub capabilities: crate::domain::types::SenderCapabilities,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PresenceResponse {
    pub device_id: DeviceId,
    pub sender_name: String,
    pub is_offline: bool,

    /// The PC's detected network link type.
    /// Sent back to the phone so it can compute the effective link pair.
    #[serde(default)]
    pub pc_network_link: Option<NetworkLink>,

    /// Whether the probing `device_id` is still in the sender's device registry.
    ///
    /// `is_offline` is a *global* sender flag, so a successful probe proves only
    /// that the PC process is up — not that this device's subscription survived.
    /// The two watchdogs make that distinction matter: the phone tears down at
    /// 10 s while the PC evicts at 15-17 s, so there is a window where the PC is
    /// still sending to a device that has already given up, and a resume is
    /// cheaper than a full `/connect`.
    ///
    /// `None` from a sender predating this field — callers must treat unknown
    /// conservatively and do a full reconnect.
    #[serde(default)]
    pub device_registered: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessListResponse {
    pub processes: Vec<crate::domain::types::ProcessInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE", tag = "type", content = "payload")]
pub enum WsEvent {
    Disconnect,
    Error { message: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE", tag = "type", content = "payload")]
pub enum WsCommand {
    Disconnect,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::types::{
        AudioSource, ConnectionMode, DeviceId, JitterConfig, SenderCapabilities,
    };

    mod connect_req {
        use super::*;

        #[test]
        fn serde_should_default_bitrate_to_128k_when_omitted() {
            let json = r#"{
                "deviceId": "phone_1",
                "deviceName": "My Phone",
                "mode": "wifi",
                "jitterConfig": {
                    "minDepthMs": 0,
                    "comfortCapMs": 0,
                    "peakDecayHalflifeMs": 1000,
                    "resumeThresholdPct": 0.0
                }
            }"#;
            let req: ConnectReq = serde_json::from_str(json).unwrap();
            assert_eq!(
                req.bitrate,
                Some(128_000),
                "Expected default bitrate of 128000"
            );
        }

        #[test]
        fn serde_should_round_trip_with_explicit_source() {
            let req = ConnectReq {
                device_id: DeviceId("dev_1".to_string()),
                device_name: "Test".to_string(),
                mode: ConnectionMode::Adb,
                jitter_config: JitterConfig::default(),
                bitrate: Some(256_000),
                source: Some(AudioSource::Process {
                    pid: 42,
                    name: "spotify.exe".to_string(),
                }),
                network_link: None,
            };
            let json = serde_json::to_string(&req).unwrap();
            let parsed: ConnectReq = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed.device_id, req.device_id);
            assert_eq!(parsed.bitrate, Some(256_000));
            assert!(matches!(
                parsed.source,
                Some(AudioSource::Process { pid: 42, .. })
            ));
        }
    }

    mod sources_response {
        use super::*;

        #[test]
        fn serde_should_round_trip() {
            let resp = SourcesResponse {
                sources: vec![
                    AudioSource::Desktop,
                    AudioSource::Process {
                        pid: 100,
                        name: "app.exe".to_string(),
                    },
                ],
                capabilities: SenderCapabilities {
                    supports_process_capture: true,
                },
            };
            let json = serde_json::to_string(&resp).unwrap();
            let parsed: SourcesResponse = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed.sources.len(), 2);
            assert!(parsed.capabilities.supports_process_capture);
        }
    }

    mod ws_event {
        use super::*;

        #[test]
        fn error_variant_should_serialize_with_screaming_snake_case_tag() {
            let event = WsEvent::Error {
                message: "something broke".to_string(),
            };
            let json = serde_json::to_string(&event).unwrap();
            assert!(
                json.contains("\"type\":\"ERROR\"") || json.contains("\"type\": \"ERROR\""),
                "Expected SCREAMING_SNAKE_CASE type tag, got: {json}"
            );
            let parsed: WsEvent = serde_json::from_str(&json).unwrap();
            assert!(matches!(parsed, WsEvent::Error { message } if message == "something broke"));
        }

        #[test]
        fn disconnect_variant_should_round_trip() {
            let event = WsEvent::Disconnect;
            let json = serde_json::to_string(&event).unwrap();
            let parsed: WsEvent = serde_json::from_str(&json).unwrap();
            assert!(matches!(parsed, WsEvent::Disconnect));
        }
    }

    mod ws_command {
        use super::*;

        #[test]
        fn disconnect_should_round_trip() {
            let cmd = WsCommand::Disconnect;
            let json = serde_json::to_string(&cmd).unwrap();
            let parsed: WsCommand = serde_json::from_str(&json).unwrap();
            assert!(matches!(parsed, WsCommand::Disconnect));
        }
    }
}
