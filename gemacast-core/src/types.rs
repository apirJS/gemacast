use serde::{Deserialize, Serialize};
use std::borrow::Borrow;
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DeviceId(pub String);

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

pub use crate::control::messages::ControlMessage;

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
