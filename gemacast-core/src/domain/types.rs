use serde::{Deserialize, Serialize};
use std::borrow::Borrow;
use std::fmt;
use std::net::SocketAddr;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DeviceId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TargetId {
    Udp(SocketAddr),
    Tcp(DeviceId),
}

impl fmt::Display for DeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for DeviceId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl Borrow<str> for DeviceId {
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl From<String> for DeviceId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl DeviceId {
    pub fn new() -> Self {
        DeviceId(format!(
            "PC_{}",
            whoami::hostname().unwrap_or("UNKNOWN".to_string())
        ))
    }
}

impl Default for DeviceId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TransportType {
    Wifi,
    Usb,
    Adb,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum AudioSource {
    #[default]
    Desktop,
    Process {
        pid: u32,
        name: String,
    },
}

/// A running process discovered on the PC sender, suitable for per-process audio capture.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessInfo {
    pub pid: u32,
    pub name: String,
    pub has_audio_session: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SenderCapabilities {
    pub supports_process_capture: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredDevice {
    pub device_id: DeviceId,
    pub device_name: String,
    pub addr: std::net::SocketAddr,
    #[serde(skip)]
    pub last_seen: std::time::Instant,
    pub is_offline: bool,
    pub transport: Option<TransportType>,
}

impl DiscoveredDevice {
    pub fn from_presence(
        sender_id: DeviceId,
        sender_name: String,
        is_offline: bool,
        addr: std::net::SocketAddr,
        transport: Option<TransportType>,
    ) -> Self {
        Self {
            device_id: sender_id,
            device_name: sender_name,
            last_seen: std::time::Instant::now(),
            addr,
            is_offline,
            transport,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ConnectionMode {
    #[default]
    Wifi,
    Usb,
    Adb,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConnectionModes {
    pub wifi: bool,
    pub usb: bool,
    pub adb: bool,
}

pub fn get_available_connection_modes() -> ConnectionModes {
    ConnectionModes {
        wifi: true,
        usb: true,
        adb: true,
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JitterConfig {
    pub min_depth_ms: u32,
    pub comfort_cap_ms: u32,
    pub peak_decay_halflife_ms: u32,
    pub resume_threshold_pct: f32,
    #[serde(default)]
    pub static_target_ms: Option<u32>,
}

impl Default for JitterConfig {
    fn default() -> Self {
        Self {
            min_depth_ms: 0,
            comfort_cap_ms: 0,
            peak_decay_halflife_ms: 1000,
            resume_threshold_pct: 0.0,
            static_target_ms: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod device_id {
        use super::*;

        #[test]
        fn display_should_output_inner_string() {
            let id = DeviceId("PC_MYHOST".to_string());
            assert_eq!(id.to_string(), "PC_MYHOST");
        }

        #[test]
        fn as_ref_should_return_inner_str() {
            let id = DeviceId("test_dev".to_string());
            let s: &str = id.as_ref();
            assert_eq!(s, "test_dev");
        }

        #[test]
        fn from_string_should_construct_device_id() {
            let id = DeviceId::from("hello".to_string());
            assert_eq!(id.0, "hello");
        }

        #[test]
        fn serde_should_round_trip_as_transparent_string() {
            let id = DeviceId("PC_123".to_string());
            let json = serde_json::to_string(&id).unwrap();
            assert_eq!(json, "\"PC_123\"");
            let parsed: DeviceId = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, id);
        }
    }

    mod audio_source {
        use super::*;

        #[test]
        fn default_should_be_desktop() {
            assert_eq!(AudioSource::default(), AudioSource::Desktop);
        }

        #[test]
        fn desktop_should_round_trip_through_json() {
            let src = AudioSource::Desktop;
            let json = serde_json::to_string(&src).unwrap();
            let parsed: AudioSource = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, AudioSource::Desktop);
        }

        #[test]
        fn process_should_round_trip_with_camel_case_fields() {
            let src = AudioSource::Process {
                pid: 1234,
                name: "chrome.exe".to_string(),
            };
            let json = serde_json::to_string(&src).unwrap();
            assert!(
                json.contains("\"type\":\"process\"") || json.contains("\"type\": \"process\""),
                "Expected tagged type field, got: {json}"
            );
            let parsed: AudioSource = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, src);
        }
    }

    mod transport_type {
        use super::*;

        #[test]
        fn serde_should_use_lowercase_variant_names() {
            let t = TransportType::Adb;
            let json = serde_json::to_string(&t).unwrap();
            assert_eq!(json, "\"adb\"");

            let parsed: TransportType = serde_json::from_str("\"wifi\"").unwrap();
            assert_eq!(parsed, TransportType::Wifi);
        }
    }

    mod connection_mode {
        use super::*;

        #[test]
        fn default_should_be_wifi() {
            assert_eq!(ConnectionMode::default(), ConnectionMode::Wifi);
        }
    }

    mod jitter_config {
        use super::*;

        #[test]
        fn default_should_have_expected_field_values() {
            let config = JitterConfig::default();
            assert_eq!(config.min_depth_ms, 0);
            assert_eq!(config.comfort_cap_ms, 0);
            assert_eq!(config.peak_decay_halflife_ms, 1000);
            assert_eq!(config.resume_threshold_pct, 0.0);
            assert!(config.static_target_ms.is_none());
        }

        #[test]
        fn serde_should_default_static_target_to_none_when_absent() {
            let json = r#"{
                "minDepthMs": 10,
                "comfortCapMs": 200,
                "peakDecayHalflifeMs": 3500,
                "resumeThresholdPct": 0.75
            }"#;
            let config: JitterConfig = serde_json::from_str(json).unwrap();
            assert_eq!(config.min_depth_ms, 10);
            assert_eq!(config.comfort_cap_ms, 200);
            assert!(config.static_target_ms.is_none());
        }

        #[test]
        fn serde_should_round_trip_with_static_target() {
            let config = JitterConfig {
                min_depth_ms: 5,
                comfort_cap_ms: 100,
                peak_decay_halflife_ms: 2000,
                resume_threshold_pct: 0.5,
                static_target_ms: Some(50),
            };
            let json = serde_json::to_string(&config).unwrap();
            let parsed: JitterConfig = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, config);
        }
    }
}
