use gemacast_core::control::SessionGeneration;
use gemacast_core::domain::types::{ConnectionMode, DeviceId, JitterConfig, NetworkLink};
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
    /// Credentials returned by the acknowledged PC connection. Required for
    /// the ADB audio socket; absent for UDP/Wi-Fi sessions.
    pub session_token: Option<String>,
    pub session_generation: Option<SessionGeneration>,
    /// Effective network link for the session, used for runtime link-aware
    /// jitter policy (e.g. reorder tolerance). Derived from the connect-time
    /// [`LinkPair`]'s `effective_link()`.
    pub network_link: NetworkLink,
}

/// Snapshot of an active session's metadata.
#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub exclusive_mode: bool,
    pub exclusive_granted: bool,
    pub mode: ConnectionMode,
    pub bitrate: Option<i32>,
    pub jitter_config: JitterConfig,
    pub target_ip: Option<IpAddr>,
    pub device_id: String,
    pub network_link: NetworkLink,
    pub session_token: Option<String>,
    pub session_generation: Option<SessionGeneration>,
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
    /// Pre-detected phone network link (from discovery service).
    pub phone_network_link: Option<NetworkLink>,
}

/// Parameters for resuming audio playback after an HTTPS reconnect.
#[derive(Debug, Clone)]
pub struct ResumeParams {
    pub ip: IpAddr,
    pub device_id: DeviceId,
    pub device_name: String,
}
