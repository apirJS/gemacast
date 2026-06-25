//! Pure service functions for discovery, decoupled from Tauri.
//!
//! These functions take trait references as parameters, making the
//! transport classification and network identity logic fully testable.

use crate::traits::{NetworkInfoProvider, PlatformService};
use gemacast_core::domain::types::ConnectionModes;

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
            match transport {
                "WIFI" => modes.wifi = true,
                "ETHERNET" => modes.usb = true,
                _ => {}
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
}
