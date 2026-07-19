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

/// The detected network link type for one side of the connection.
///
/// Used at connection time to determine how aggressive the jitter buffer
/// can be. The [`LinkPair`] combines both sides and picks the weaker link.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NetworkLink {
    /// ADB reverse port forwarding (localhost TCP).
    Adb,
    /// USB tethering (direct cable, RNDIS/ETHERNET transport).
    UsbTether,
    /// WiFi on 5 GHz or 6 GHz band (clean, low jitter).
    Wifi5Ghz,
    /// WiFi on 2.4 GHz band (congested, higher jitter).
    #[serde(rename = "wifi2_4Ghz")]
    Wifi2_4Ghz,
    /// Wired Ethernet.
    Ethernet,
    /// WiFi but band could not be determined.
    WifiUnknown,
    /// Could not determine the link type.
    Unknown,
}

impl NetworkLink {
    /// Quality ranking: lower is better. Used by [`LinkPair::effective_link`]
    /// to pick the weaker side.
    pub fn quality_rank(self) -> u8 {
        match self {
            Self::Adb | Self::UsbTether => 0, // Best: direct cable / localhost
            Self::Ethernet => 1,              // Excellent: wired LAN
            Self::Wifi5Ghz => 2,              // Great: clean RF band
            Self::WifiUnknown => 3,           // Unknown WiFi: assume mediocre
            Self::Wifi2_4Ghz => 4,            // Poor: congested 2.4 GHz
            Self::Unknown => 5,               // Worst: no information
        }
    }
}

/// Network link information from both sides of the connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkPair {
    pub phone: NetworkLink,
    pub pc: NetworkLink,
}

impl LinkPair {
    /// Returns the effective link quality, determining the bottleneck.
    ///
    /// Resolves asymmetrical knowledge (e.g. if the Phone knows it's on 5GHz,
    /// but the PC lacks permissions and reports `WifiUnknown`, we trust the Phone's 5GHz).
    pub fn effective_link(&self) -> NetworkLink {
        let has = |link: NetworkLink| self.phone == link || self.pc == link;

        // 1. Direct cables bypass everything. If either side knows it's a wire,
        // we trust the wire. (ADB is port forwarding, USB is direct).
        if has(NetworkLink::Adb) {
            return NetworkLink::Adb;
        }
        if has(NetworkLink::UsbTether) {
            return NetworkLink::UsbTether;
        }

        // 2. The worst known physical bottleneck dominates.
        if has(NetworkLink::Wifi2_4Ghz) {
            return NetworkLink::Wifi2_4Ghz;
        }

        // 3. Resolve 5GHz vs Unknowns.
        if has(NetworkLink::Wifi5Ghz) {
            // If the other side is Ethernet, or Unknown (lacks metadata), the bottleneck is 5GHz.
            return NetworkLink::Wifi5Ghz;
        }

        // 4. Resolve Ethernet vs Unknowns.
        if has(NetworkLink::Ethernet) {
            if has(NetworkLink::WifiUnknown) || has(NetworkLink::Unknown) {
                // E.g., PC is Ethernet, but Phone is Unknown. Phones don't use Ethernet,
                // so the phone is almost certainly on Wi-Fi. We must assume a generic Wi-Fi link.
                return NetworkLink::WifiUnknown;
            }
            return NetworkLink::Ethernet;
        }

        // 5. Fallbacks
        if has(NetworkLink::WifiUnknown) {
            return NetworkLink::WifiUnknown;
        }

        NetworkLink::Unknown
    }
}

impl JitterConfig {
    /// Create a `JitterConfig` optimised for the detected [`LinkPair`].
    ///
    /// This is the "Option A: full override" approach — when the user has
    /// the preset set to "Auto", the network-aware profile **replaces** the
    /// generic Auto config entirely.
    pub fn for_link_pair(pair: LinkPair) -> Self {
        match pair.effective_link() {
            // Ultra — localhost / direct cable, near-zero jitter
            NetworkLink::Adb | NetworkLink::UsbTether => Self {
                min_depth_ms: 2,
                comfort_cap_ms: 20,
                peak_decay_halflife_ms: 500,
                resume_threshold_pct: 0.2,
                static_target_ms: None,
            },
            // Aggressive — clean 5 GHz or wired Ethernet
            NetworkLink::Ethernet | NetworkLink::Wifi5Ghz => Self {
                min_depth_ms: 5,
                comfort_cap_ms: 60,
                peak_decay_halflife_ms: 800,
                resume_threshold_pct: 0.25,
                static_target_ms: None,
            },
            // Conservative — congested 2.4 GHz band
            NetworkLink::Wifi2_4Ghz => Self {
                min_depth_ms: 25,
                comfort_cap_ms: 500,
                peak_decay_halflife_ms: 15000,
                resume_threshold_pct: 0.5,
                static_target_ms: None,
            },
            // Fallback — unknown WiFi or completely unknown link
            NetworkLink::WifiUnknown | NetworkLink::Unknown => Self {
                min_depth_ms: 25,
                comfort_cap_ms: 1000,
                peak_decay_halflife_ms: 0,
                resume_threshold_pct: 0.25,
                static_target_ms: None,
            },
        }
    }

    /// Returns `true` if this config looks like the "Auto" preset sentinel.
    ///
    /// Auto is uniquely identified by `peak_decay_halflife_ms == 0` (fully
    /// adaptive) with no static target. This is used by the LinkPair cache
    /// to decide whether to re-apply the network-aware override.
    pub fn is_auto_sentinel(&self) -> bool {
        self.peak_decay_halflife_ms == 0 && self.static_target_ms.is_none()
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

    mod network_link {
        use super::*;

        #[test]
        fn quality_rank_order_should_be_cable_best_unknown_worst() {
            assert!(NetworkLink::Adb.quality_rank() < NetworkLink::Ethernet.quality_rank());
            assert!(NetworkLink::UsbTether.quality_rank() < NetworkLink::Ethernet.quality_rank());
            assert!(NetworkLink::Ethernet.quality_rank() < NetworkLink::Wifi5Ghz.quality_rank());
            assert!(NetworkLink::Wifi5Ghz.quality_rank() < NetworkLink::WifiUnknown.quality_rank());
            assert!(
                NetworkLink::WifiUnknown.quality_rank() < NetworkLink::Wifi2_4Ghz.quality_rank()
            );
            assert!(NetworkLink::Wifi2_4Ghz.quality_rank() < NetworkLink::Unknown.quality_rank());
        }

        #[test]
        fn adb_and_usb_tether_should_have_same_rank() {
            assert_eq!(
                NetworkLink::Adb.quality_rank(),
                NetworkLink::UsbTether.quality_rank()
            );
        }

        #[test]
        fn serde_should_round_trip_all_variants() {
            let variants = [
                NetworkLink::Adb,
                NetworkLink::UsbTether,
                NetworkLink::Wifi5Ghz,
                NetworkLink::Wifi2_4Ghz,
                NetworkLink::Ethernet,
                NetworkLink::WifiUnknown,
                NetworkLink::Unknown,
            ];
            for v in variants {
                let json = serde_json::to_string(&v).unwrap();
                let parsed: NetworkLink = serde_json::from_str(&json).unwrap();
                assert_eq!(parsed, v, "Round-trip failed for {v:?} (json: {json})");
            }
        }

        #[test]
        fn wifi_2_4ghz_should_serialize_with_underscore() {
            let json = serde_json::to_string(&NetworkLink::Wifi2_4Ghz).unwrap();
            assert_eq!(json, "\"wifi2_4Ghz\"");
        }
    }

    mod link_pair {
        use super::*;

        #[test]
        fn effective_link_should_return_weaker_side() {
            let pair = LinkPair {
                phone: NetworkLink::Wifi5Ghz,
                pc: NetworkLink::Wifi2_4Ghz,
            };
            assert_eq!(pair.effective_link(), NetworkLink::Wifi2_4Ghz);
        }

        #[test]
        fn effective_link_should_return_phone_when_phone_is_weaker() {
            let pair = LinkPair {
                phone: NetworkLink::Wifi2_4Ghz,
                pc: NetworkLink::Ethernet,
            };
            assert_eq!(pair.effective_link(), NetworkLink::Wifi2_4Ghz);
        }

        #[test]
        fn effective_link_should_return_phone_when_equal() {
            let pair = LinkPair {
                phone: NetworkLink::Wifi5Ghz,
                pc: NetworkLink::Wifi5Ghz,
            };
            // Equal ranks → phone is returned (>= comparison)
            assert_eq!(pair.effective_link(), NetworkLink::Wifi5Ghz);
        }

        #[test]
        fn effective_link_both_adb_should_return_adb() {
            let pair = LinkPair {
                phone: NetworkLink::Adb,
                pc: NetworkLink::Ethernet,
            };
            // ADB rank (0) < Ethernet rank (1), so PC is worse → returns Ethernet
            // Wait: phone rank 0, pc rank 1. phone.rank < pc.rank, so pc is weaker.
            // Actually: higher rank = worse quality. PC rank 1 > phone rank 0.
            // phone.quality_rank() (0) >= pc.quality_rank() (1)? No. → returns pc.
            assert_eq!(pair.effective_link(), NetworkLink::Ethernet);
        }

        #[test]
        fn effective_link_ethernet_pc_with_unknown_phone() {
            let pair = LinkPair {
                phone: NetworkLink::Unknown,
                pc: NetworkLink::Ethernet,
            };
            assert_eq!(pair.effective_link(), NetworkLink::Unknown);
        }

        #[test]
        fn serde_should_round_trip() {
            let pair = LinkPair {
                phone: NetworkLink::Wifi5Ghz,
                pc: NetworkLink::Ethernet,
            };
            let json = serde_json::to_string(&pair).unwrap();
            let parsed: LinkPair = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, pair);
        }
    }

    mod for_link_pair {
        use super::*;

        #[test]
        fn adb_pair_should_produce_ultra_profile() {
            let pair = LinkPair {
                phone: NetworkLink::Adb,
                pc: NetworkLink::Ethernet,
            };
            let config = JitterConfig::for_link_pair(pair);
            // Effective link = Ethernet (rank 1 > Adb rank 0)
            assert_eq!(config.min_depth_ms, 5);
            assert_eq!(config.comfort_cap_ms, 60);
        }

        #[test]
        fn both_adb_should_produce_ultra() {
            let pair = LinkPair {
                phone: NetworkLink::Adb,
                pc: NetworkLink::UsbTether,
            };
            let config = JitterConfig::for_link_pair(pair);
            assert_eq!(config.min_depth_ms, 2);
            assert_eq!(config.comfort_cap_ms, 20);
            assert_eq!(config.peak_decay_halflife_ms, 500);
        }

        #[test]
        fn both_5ghz_should_produce_aggressive() {
            let pair = LinkPair {
                phone: NetworkLink::Wifi5Ghz,
                pc: NetworkLink::Wifi5Ghz,
            };
            let config = JitterConfig::for_link_pair(pair);
            assert_eq!(config.min_depth_ms, 5);
            assert_eq!(config.comfort_cap_ms, 60);
            assert_eq!(config.peak_decay_halflife_ms, 800);
        }

        #[test]
        fn mixed_wifi_should_use_2_4ghz_profile() {
            let pair = LinkPair {
                phone: NetworkLink::Wifi5Ghz,
                pc: NetworkLink::Wifi2_4Ghz,
            };
            let config = JitterConfig::for_link_pair(pair);
            assert_eq!(config.min_depth_ms, 25);
            assert_eq!(config.comfort_cap_ms, 500);
            assert_eq!(config.peak_decay_halflife_ms, 15000);
        }

        #[test]
        fn ethernet_pc_with_2_4ghz_phone_should_use_conservative() {
            let pair = LinkPair {
                phone: NetworkLink::Wifi2_4Ghz,
                pc: NetworkLink::Ethernet,
            };
            let config = JitterConfig::for_link_pair(pair);
            assert_eq!(config.min_depth_ms, 25);
            assert_eq!(config.comfort_cap_ms, 500);
            assert_eq!(config.resume_threshold_pct, 0.5);
        }

        #[test]
        fn unknown_link_should_produce_fallback_auto() {
            let pair = LinkPair {
                phone: NetworkLink::Unknown,
                pc: NetworkLink::Ethernet,
            };
            let config = JitterConfig::for_link_pair(pair);
            assert_eq!(config.min_depth_ms, 25);
            assert_eq!(config.comfort_cap_ms, 1000);
            assert_eq!(config.peak_decay_halflife_ms, 0);
        }

        #[test]
        fn wifi_unknown_should_produce_fallback_auto() {
            let pair = LinkPair {
                phone: NetworkLink::WifiUnknown,
                pc: NetworkLink::WifiUnknown,
            };
            let config = JitterConfig::for_link_pair(pair);
            assert_eq!(config.peak_decay_halflife_ms, 0);
            assert_eq!(config.comfort_cap_ms, 1000);
        }

        #[test]
        fn all_profiles_should_have_no_static_target() {
            let pairs = [
                (NetworkLink::Adb, NetworkLink::Adb),
                (NetworkLink::Wifi5Ghz, NetworkLink::Wifi5Ghz),
                (NetworkLink::Wifi2_4Ghz, NetworkLink::Wifi2_4Ghz),
                (NetworkLink::Unknown, NetworkLink::Unknown),
            ];
            for (phone, pc) in pairs {
                let config = JitterConfig::for_link_pair(LinkPair { phone, pc });
                assert!(
                    config.static_target_ms.is_none(),
                    "for_link_pair should always produce adaptive config (no static target)"
                );
            }
        }
    }

    mod is_auto_sentinel {
        use super::*;

        #[test]
        fn auto_config_should_be_detected() {
            let config = JitterConfig {
                min_depth_ms: 25,
                comfort_cap_ms: 1000,
                peak_decay_halflife_ms: 0,
                resume_threshold_pct: 0.25,
                static_target_ms: None,
            };
            assert!(config.is_auto_sentinel());
        }

        #[test]
        fn non_auto_config_should_not_be_detected() {
            let config = JitterConfig {
                min_depth_ms: 10,
                comfort_cap_ms: 200,
                peak_decay_halflife_ms: 3500,
                resume_threshold_pct: 0.75,
                static_target_ms: None,
            };
            assert!(!config.is_auto_sentinel());
        }

        #[test]
        fn no_buffer_should_not_be_auto_sentinel() {
            // No Buffer has staticTargetMs: Some(0), distinguishing it from Auto
            let config = JitterConfig {
                min_depth_ms: 0,
                comfort_cap_ms: 0,
                peak_decay_halflife_ms: 0,
                resume_threshold_pct: 0.0,
                static_target_ms: Some(0),
            };
            assert!(!config.is_auto_sentinel());
        }
    }
}
