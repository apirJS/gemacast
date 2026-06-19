use gemacast_core::types::{ConnectionMode, DeviceId, JitterConfig};
use std::net::IpAddr;

/// Parameters for starting a new audio playback session.
#[derive(Debug, Clone)]
pub struct SessionParams {
    pub jitter_config: JitterConfig,
    pub is_tcp: bool,
    pub exclusive_mode: bool,
    pub target_ip: Option<IpAddr>,
    pub mode: ConnectionMode,
    pub device_id: String,
    pub bitrate: Option<i32>,
}

/// Snapshot of an active session's metadata.
#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub exclusive_mode: bool,
    pub mode: ConnectionMode,
    pub bitrate: Option<i32>,
    pub jitter_config: JitterConfig,
}

/// Simplified network interface info, decoupled from `netdev::Interface`.
#[derive(Debug, Clone)]
pub struct InterfaceInfo {
    pub name: String,
    pub mac_addr: Option<String>,
    pub ipv4: Vec<std::net::Ipv4Addr>,
    pub ipv6: Vec<std::net::Ipv6Addr>,
    pub is_wifi: bool,
    pub is_usb: bool,
}

/// Parameters for connecting to a sender.
#[derive(Debug, Clone)]
pub struct ConnectParams {
    pub ip: String,
    pub device_id: DeviceId,
    pub device_name: String,
    pub mode: ConnectionMode,
    pub exclusive_mode: bool,
    pub jitter_config: JitterConfig,
    pub bitrate: Option<i32>,
}

/// Parameters for resuming audio playback with an HTTP reconnect.
#[derive(Debug, Clone)]
pub struct ResumeParams {
    pub ip: IpAddr,
    pub device_id: DeviceId,
    pub device_name: String,
}
