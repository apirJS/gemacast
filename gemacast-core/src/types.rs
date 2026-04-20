use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ControlMessage {
    Probe {
        #[serde(default)]
        device_id: Option<String>,
    },
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
        #[serde(default)]
        mode: ConnectionMode,
        #[serde(default)]
        exclusive_mode: bool,
        #[serde(default)]
        jitter_config: JitterConfig,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum ConnectionMode {
    #[default]
    Wifi,
    Usb,
    Adb,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JitterConfig {
    pub min_depth_ms: u32,
    pub comfort_cap_ms: u32,
    pub bounce_multiplier: f32,
    pub resume_threshold_pct: f32, // e.g., 0.75 for 75%
    pub wsola_max_skip: usize,
    /// Starting comfort point on fresh connect/reset.
    /// Seeds the bouncer at a known-good level instead of
    /// discovering it from scratch via starvation cycles.
    #[serde(default = "default_initial_comfort_ms")]
    pub initial_comfort_ms: u32,
    /// Multiplier for bleed rate during the first N frames after reset.
    /// Values > 1.0 make the system converge faster to optimal latency.
    #[serde(default = "default_fast_settle_multiplier")]
    pub fast_settle_multiplier: f32,
    /// How many frames the fast-settle period lasts after a reset.
    #[serde(default = "default_fast_settle_frames")]
    pub fast_settle_frames: u32,
}

fn default_initial_comfort_ms() -> u32 { 50 }
fn default_fast_settle_multiplier() -> f32 { 2.5 }
fn default_fast_settle_frames() -> u32 { 200 }

impl Default for JitterConfig {
    fn default() -> Self {
        // Preset Level 5 (Stable) defaults.
        Self {
            min_depth_ms: 40,
            comfort_cap_ms: 280,
            bounce_multiplier: 1.5,
            resume_threshold_pct: 0.50,
            wsola_max_skip: 2,
            initial_comfort_ms: default_initial_comfort_ms(),
            fast_settle_multiplier: default_fast_settle_multiplier(),
            fast_settle_frames: default_fast_settle_frames(),
        }
    }
}
