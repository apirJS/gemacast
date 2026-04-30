use serde::{Deserialize, Serialize};
use std::borrow::Borrow;
use std::fmt;

/// Strongly-typed wrapper for device identifiers (mobile/receiver side).
///
/// Serializes transparently as a raw JSON string for wire compatibility.
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

/// Strongly-typed wrapper for sender identifiers (PC/broadcaster side).
///
/// Serializes transparently as a raw JSON string for wire compatibility.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SenderId(pub String);

impl fmt::Display for SenderId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for SenderId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl Borrow<str> for SenderId {
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl From<String> for SenderId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

/// The physical network transport a presence announcement was received over.
///
/// This is distinct from [`ConnectionMode`] which describes the user's chosen
/// connection strategy. `TransportType` is a runtime observation embedded in
/// `Presence` and `Connect` messages by the mobile client's native transport
/// detection layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TransportType {
    Wifi,
    Usb,
}

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
    },
    Disconnect {
        device_id: DeviceId,
    },
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
        sender_id: SenderId,
        sender_name: String,
        is_offline: bool,
        addr: std::net::SocketAddr,
        transport: Option<TransportType>,
    ) -> Self {
        Self {
            device_id: DeviceId(sender_id.0),
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

/// Runtime availability of each connection mode, as reported to the frontend.
#[derive(Debug, Clone, Serialize)]
pub struct ConnectionModes {
    pub wifi: bool,
    pub usb: bool,
    pub adb: bool,
}

/// Returns the default connection mode availability (all enabled).
/// Platform-specific detection narrows this down at runtime.
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
    pub resume_threshold_pct: f32, // e.g., 0.75 for 75%
    /// When set, bypasses all adaptive EMA math and locks the buffer
    /// to this exact depth in milliseconds. `None` = adaptive mode.
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
