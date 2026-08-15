//! Pure service functions for discovery, decoupled from Tauri.
//!
//! These functions take trait references as parameters, making the
//! transport classification and network identity logic fully testable.

use crate::traits::{NetworkInfoProvider, PlatformService};
use gemacast_core::domain::types::{ConnectionModes, DeviceId, NetworkLink};

/// Network state returned to the frontend.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkState {
    pub local_ip: String,
    pub network_id: String,
    pub modes: ConnectionModes,
}

/// Get the local IP address as a string.
pub fn get_local_ip(network: &dyn NetworkInfoProvider) -> Result<String, String> {
    network.get_local_ip().map(|ip| ip.to_string())
}

/// Build a network identifier string from the default interface.
///
/// Format: `"{interface_name}_{mac}_{ip}"`.
pub fn get_network_identifier(network: &dyn NetworkInfoProvider) -> Result<String, String> {
    let iface = network.get_default_interface()?;
    let mac = iface
        .mac_addr
        .unwrap_or_else(|| "00:00:00:00:00:00".to_string());
    let ip = if let Some(ip) = iface.ipv4.first() {
        std::net::IpAddr::V4(*ip).to_string()
    } else if let Some(ip) = iface.ipv6.first() {
        std::net::IpAddr::V6(*ip).to_string()
    } else {
        "no-ip".to_string()
    };
    Ok(format!("{}_{}_{}", iface.name, mac, ip))
}

/// Determine which connection modes are available.
///
/// Checks the platform transport type (Android JNI) and enriches
/// with local network interface information.
pub fn get_connection_status(
    network: &dyn NetworkInfoProvider,
    platform: &dyn PlatformService,
) -> Result<ConnectionModes, String> {
    let mut modes = gemacast_core::domain::types::get_available_connection_modes();

    // Platform-specific transport detection (Android JNI)
    if let Ok(transport_str) = platform.get_transport_type() {
        modes.wifi = false;
        modes.usb = false;

        let parts: Vec<&str> = transport_str.split('|').collect();
        let network_type = parts.first().unwrap_or(&"");
        let adb_status = parts.get(1).unwrap_or(&"");

        if *adb_status == "ADB_OFF" {
            modes.adb = false;
        }

        for transport in network_type.split(',') {
            if transport == "WIFI" || transport.starts_with("WIFI:") {
                modes.wifi = true;
            } else if transport == "ETHERNET" {
                modes.usb = true;
            }
        }
    }

    // Enrich with interface classification
    let interfaces = network.get_interfaces();
    for iface in interfaces {
        if iface.is_wifi && !iface.ipv4.is_empty() {
            modes.wifi = true;
        }
        if iface.is_usb && !iface.ipv4.is_empty() {
            modes.usb = true;
        }
    }

    Ok(modes)
}

/// Get a combined network state snapshot.
pub fn get_network_state(
    network: &dyn NetworkInfoProvider,
    platform: &dyn PlatformService,
) -> Result<NetworkState, String> {
    let local_ip = network
        .get_local_ip()
        .map(|ip| ip.to_string())
        .unwrap_or_else(|_| "127.0.0.1".to_string());

    let network_id = get_network_identifier(network).unwrap_or_else(|_| local_ip.clone());

    let modes = get_connection_status(network, platform).unwrap_or(ConnectionModes {
        wifi: true,
        usb: false,
        adb: false,
    });

    Ok(NetworkState {
        local_ip,
        network_id,
        modes,
    })
}

/// Remove one locally stored PC certificate pin.
pub fn forget_pc_identity(platform: &dyn PlatformService, pc_id: &DeviceId) -> Result<(), String> {
    platform.forget_pc_identity(pc_id)
}

/// Return the locally stored PC identities used by the paired-PC settings UI.
pub fn paired_pc_ids(platform: &dyn PlatformService) -> Result<Vec<DeviceId>, String> {
    platform.paired_pc_ids()
}

/// Detect the phone's network link type from platform transport info.
///
/// Combines the user-selected connection mode with the Android JNI transport
/// string (e.g., `"WIFI:5180|ADB_ON"`) to determine the best description
/// of the phone's network link quality.
pub fn detect_phone_link(
    network: &dyn NetworkInfoProvider,
    platform: &dyn PlatformService,
    mode: &str, // "wifi", "usb", "adb"
) -> NetworkLink {
    let transport_str = platform.get_transport_type().unwrap_or_default();
    tracing::info!(
        mode,
        transport = %transport_str,
        "Detecting phone network link"
    );

    // 1. ADB mode → Always ADB
    if mode == "adb" {
        tracing::info!(link = ?NetworkLink::Adb, "Phone link detected (ADB mode)");
        return NetworkLink::Adb;
    }

    // 2. USB mode → Always USB Tether
    if mode == "usb" {
        tracing::info!(link = ?NetworkLink::UsbTether, "Phone link detected (USB mode)");
        return NetworkLink::UsbTether;
    }

    // 3. WIFI mode → Only look for WiFi transports from Android JNI
    if mode == "wifi" && !transport_str.is_empty() {
        let parts: Vec<&str> = transport_str.split('|').collect();
        let network_type = parts.first().unwrap_or(&"");

        let mut best_wifi: Option<(NetworkLink, u32)> = None;

        for transport in network_type.split(',') {
            // "WIFI:5180" → parse frequency in MHz
            if let Some(freq_str) = transport.strip_prefix("WIFI:") {
                if let Ok(freq) = freq_str.parse::<u32>() {
                    let link = if freq >= 4000 {
                        NetworkLink::Wifi5Ghz
                    } else {
                        NetworkLink::Wifi2_4Ghz
                    };
                    best_wifi = Some((link, freq));
                } else if best_wifi.is_none() {
                    best_wifi = Some((NetworkLink::WifiUnknown, 0));
                }
            } else if transport == "WIFI" && best_wifi.is_none() {
                best_wifi = Some((NetworkLink::WifiUnknown, 0));
            }
        }

        if let Some((link, freq)) = best_wifi {
            if freq > 0 {
                tracing::info!(
                    ?link,
                    freq_mhz = freq,
                    "Phone link detected (WiFi frequency)"
                );
            } else {
                tracing::info!(?link, "Phone link detected (WiFi, no freq)");
            }
            return link;
        }
    }

    // 4. Fallback: check interfaces for WiFi/USB
    let interfaces = network.get_interfaces();
    for iface in interfaces {
        if iface.is_usb && !iface.ipv4.is_empty() {
            tracing::info!(link = ?NetworkLink::UsbTether, iface = %iface.name, "Phone link detected (interface)");
            return NetworkLink::UsbTether;
        }
        if iface.is_wifi && !iface.ipv4.is_empty() {
            tracing::info!(link = ?NetworkLink::WifiUnknown, iface = %iface.name, "Phone link detected (interface)");
            return NetworkLink::WifiUnknown;
        }
    }

    tracing::warn!("Could not detect phone network link, falling back to Unknown");
    NetworkLink::Unknown
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::mocks::*;
    use crate::traits::InterfaceInfo;

    #[test]
    fn get_network_identifier_formats_correctly() {
        let network = MockNetworkInfoProvider::new().with_default_interface(InterfaceInfo {
            name: "wlan0".to_string(),
            mac_addr: Some("AA:BB:CC:DD:EE:FF".to_string()),
            ipv4: vec!["192.168.1.100".parse().unwrap()],
            ipv6: vec![],
            is_wifi: true,
            is_usb: false,
        });

        let result = get_network_identifier(&network).unwrap();
        assert_eq!(result, "wlan0_AA:BB:CC:DD:EE:FF_192.168.1.100");
    }

    #[test]
    fn get_network_identifier_uses_default_mac_when_missing() {
        let network = MockNetworkInfoProvider::new().with_default_interface(InterfaceInfo {
            name: "eth0".to_string(),
            mac_addr: None,
            ipv4: vec!["10.0.0.1".parse().unwrap()],
            ipv6: vec![],
            is_wifi: false,
            is_usb: false,
        });

        let result = get_network_identifier(&network).unwrap();
        assert!(result.contains("00:00:00:00:00:00"));
    }

    #[test]
    fn get_connection_status_parses_wifi_transport() {
        let network = MockNetworkInfoProvider::new();
        let platform = MockPlatformService::new().with_transport_type("WIFI|ADB_ON");

        let modes = get_connection_status(&network, &platform).unwrap();
        assert!(modes.wifi);
        assert!(!modes.usb);
        assert!(modes.adb);
    }

    #[test]
    fn forget_pc_identity_delegates_to_platform() {
        let platform = MockPlatformService::new();
        let pc_id = DeviceId("pc-1".into());
        forget_pc_identity(&platform, &pc_id).unwrap();
        assert!(platform.calls.lock().unwrap().iter().any(|call| {
            matches!(call, PlatformCall::ForgetPcIdentity { pc_id: observed } if observed == &pc_id)
        }));
    }

    #[test]
    fn paired_pc_ids_delegates_to_platform() {
        let expected = vec![DeviceId("pc-1".into()), DeviceId("pc-2".into())];
        let platform = MockPlatformService::new().with_paired_pc_ids(expected.clone());

        assert_eq!(paired_pc_ids(&platform).unwrap(), expected);
        assert!(
            platform
                .calls
                .lock()
                .unwrap()
                .iter()
                .any(|call| matches!(call, PlatformCall::PairedPcIds))
        );
    }

    #[test]
    fn get_connection_status_parses_adb_off() {
        let network = MockNetworkInfoProvider::new();
        let platform = MockPlatformService::new().with_transport_type("WIFI|ADB_OFF");

        let modes = get_connection_status(&network, &platform).unwrap();
        assert!(modes.wifi);
        assert!(!modes.adb);
    }

    #[test]
    fn get_connection_status_detects_ethernet_as_usb() {
        let network = MockNetworkInfoProvider::new();
        let platform = MockPlatformService::new().with_transport_type("ETHERNET|ADB_ON");

        let modes = get_connection_status(&network, &platform).unwrap();
        assert!(!modes.wifi);
        assert!(modes.usb);
    }

    #[test]
    fn get_connection_status_enriches_from_interfaces() {
        let network = MockNetworkInfoProvider::new().with_interfaces(vec![InterfaceInfo {
            name: "wlan0".to_string(),
            mac_addr: None,
            ipv4: vec!["192.168.1.100".parse().unwrap()],
            ipv6: vec![],
            is_wifi: true,
            is_usb: false,
        }]);
        let platform = MockPlatformService::new(); // get_transport_type returns Err

        let modes = get_connection_status(&network, &platform).unwrap();
        assert!(modes.wifi);
    }

    #[test]
    fn get_connection_status_parses_combined_transports() {
        let network = MockNetworkInfoProvider::new();
        let platform = MockPlatformService::new().with_transport_type("WIFI,ETHERNET|ADB_ON");

        let modes = get_connection_status(&network, &platform).unwrap();
        assert!(modes.wifi);
        assert!(modes.usb);
        assert!(modes.adb);
    }

    #[test]
    fn get_connection_status_handles_wifi_with_frequency() {
        let network = MockNetworkInfoProvider::new();
        let platform = MockPlatformService::new().with_transport_type("WIFI:5180|ADB_OFF");

        let modes = get_connection_status(&network, &platform).unwrap();
        assert!(modes.wifi, "WIFI:5180 should still enable wifi mode");
        assert!(!modes.adb);
    }

    // ---------------------------------------------------------------
    // detect_phone_link
    // ---------------------------------------------------------------

    #[test]
    fn detect_phone_link_adb_mode_returns_adb() {
        let network = MockNetworkInfoProvider::new();
        let platform = MockPlatformService::new();

        let link = detect_phone_link(&network, &platform, "adb");
        assert_eq!(link, NetworkLink::Adb);
    }

    #[test]
    fn detect_phone_link_wifi_5ghz_frequency() {
        let network = MockNetworkInfoProvider::new();
        let platform = MockPlatformService::new().with_transport_type("WIFI:5180|ADB_OFF");

        let link = detect_phone_link(&network, &platform, "wifi");
        assert_eq!(link, NetworkLink::Wifi5Ghz);
    }

    #[test]
    fn detect_phone_link_wifi_2_4ghz_frequency() {
        let network = MockNetworkInfoProvider::new();
        let platform = MockPlatformService::new().with_transport_type("WIFI:2412|ADB_OFF");

        let link = detect_phone_link(&network, &platform, "wifi");
        assert_eq!(link, NetworkLink::Wifi2_4Ghz);
    }

    #[test]
    fn detect_phone_link_wifi_6ghz_treated_as_5ghz() {
        let network = MockNetworkInfoProvider::new();
        // 6 GHz channel, e.g., 5955 MHz
        let platform = MockPlatformService::new().with_transport_type("WIFI:5955|ADB_OFF");

        let link = detect_phone_link(&network, &platform, "wifi");
        assert_eq!(link, NetworkLink::Wifi5Ghz);
    }

    #[test]
    fn detect_phone_link_wifi_no_frequency() {
        let network = MockNetworkInfoProvider::new();
        let platform = MockPlatformService::new().with_transport_type("WIFI|ADB_OFF");

        let link = detect_phone_link(&network, &platform, "wifi");
        assert_eq!(link, NetworkLink::WifiUnknown);
    }

    #[test]
    fn detect_phone_link_ethernet_transport_is_usb_tether() {
        let network = MockNetworkInfoProvider::new();
        let platform = MockPlatformService::new().with_transport_type("ETHERNET|ADB_ON");

        let link = detect_phone_link(&network, &platform, "usb");
        assert_eq!(link, NetworkLink::UsbTether);
    }

    #[test]
    fn detect_phone_link_usb_mode_without_transport() {
        let network = MockNetworkInfoProvider::new();
        let platform = MockPlatformService::new(); // get_transport_type returns Err

        let link = detect_phone_link(&network, &platform, "usb");
        assert_eq!(link, NetworkLink::UsbTether);
    }

    #[test]
    fn detect_phone_link_fallback_to_interfaces() {
        let network = MockNetworkInfoProvider::new().with_interfaces(vec![InterfaceInfo {
            name: "wlan0".to_string(),
            mac_addr: None,
            ipv4: vec!["192.168.1.100".parse().unwrap()],
            ipv6: vec![],
            is_wifi: true,
            is_usb: false,
        }]);
        let platform = MockPlatformService::new(); // get_transport_type returns Err

        let link = detect_phone_link(&network, &platform, "wifi");
        assert_eq!(link, NetworkLink::WifiUnknown);
    }

    #[test]
    fn detect_phone_link_no_info_returns_unknown() {
        let network = MockNetworkInfoProvider::new();
        let platform = MockPlatformService::new(); // get_transport_type returns Err

        let link = detect_phone_link(&network, &platform, "wifi");
        assert_eq!(link, NetworkLink::Unknown);
    }
}
