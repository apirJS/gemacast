use std::net::IpAddr;
use crate::traits::{InterfaceInfo, NetworkInfoProvider};

/// Network info from the real OS via `netdev` and `gemacast_core::network`.
pub struct NativeNetworkInfoProvider;

impl NetworkInfoProvider for NativeNetworkInfoProvider {
    fn get_local_ip(&self) -> Result<IpAddr, String> {
        gemacast_core::network::get_local_ip().map_err(|e| e.to_string())
    }

    fn get_default_interface(&self) -> Result<InterfaceInfo, String> {
        let iface = netdev::get_default_interface().map_err(|e| e.to_string())?;
        Ok(to_interface_info(&iface))
    }

    fn get_interfaces(&self) -> Vec<InterfaceInfo> {
        netdev::get_interfaces()
            .iter()
            .map(to_interface_info)
            .collect()
    }
}

fn to_interface_info(iface: &netdev::Interface) -> InterfaceInfo {
    let (is_wifi, is_usb) = gemacast_core::network::classify_interface(iface);
    InterfaceInfo {
        name: iface.name.clone(),
        mac_addr: iface.mac_addr.map(|m| m.to_string()),
        ipv4: iface.ipv4.iter().map(|net| net.addr()).collect(),
        ipv6: iface.ipv6.iter().map(|net| net.addr()).collect(),
        is_wifi,
        is_usb,
    }
}
