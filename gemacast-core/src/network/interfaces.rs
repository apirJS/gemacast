use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Mutex;
use std::time::Instant;

use crate::domain::error::NetworkError;

use super::InterfaceClassifier;

const CACHE_TTL_SECONDS: u64 = 5;

static INTERFACE_CACHE: Mutex<Option<CachedNetworkInterfaces>> = Mutex::new(None);

struct CachedNetworkInterfaces {
    interfaces: Vec<netdev::Interface>,
    refreshed_at: Instant,
}

pub struct NetworkInterfaces;

impl NetworkInterfaces {
    pub fn primary_ip() -> Result<IpAddr, NetworkError> {
        let interfaces = Self::all();

        interfaces
            .iter()
            .flat_map(|interface| interface.ipv4.iter())
            .map(|network| IpAddr::V4(network.addr()))
            .find(|address| !address.is_loopback())
            .or_else(|| {
                interfaces
                    .iter()
                    .flat_map(|interface| interface.ipv6.iter())
                    .map(|network| IpAddr::V6(network.addr()))
                    .find(|address| !address.is_loopback())
            })
            .ok_or(NetworkError::LocalAddressUnavailable)
    }

    pub fn broadcast_addresses() -> Vec<Ipv4Addr> {
        let mut addresses = Vec::new();

        for interface in Self::all() {
            for network in interface.ipv4 {
                let address = network.addr();
                if !address.is_loopback() {
                    Self::add_class_c_broadcast(&mut addresses, address);
                }
            }
        }

        if let Some(address) = Self::default_route_ipv4() {
            Self::add_class_c_broadcast(&mut addresses, address);
        }

        if addresses.is_empty() {
            addresses.push(Ipv4Addr::BROADCAST);
        }

        addresses
    }

    pub fn is_usb_tether_ip(address: &IpAddr) -> bool {
        let IpAddr::V4(address) = address else {
            return false;
        };

        InterfaceClassifier::is_usb_tether_address(address)
            || Self::all().iter().any(|interface| {
                InterfaceClassifier::classify(interface).supports_usb_tethering()
                    && interface
                        .ipv4
                        .iter()
                        .any(|network| Self::shares_class_c_subnet(network.addr(), *address))
            })
    }

    pub(crate) fn all() -> Vec<netdev::Interface> {
        let mut cache = INTERFACE_CACHE
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let now = Instant::now();

        if let Some(snapshot) = cache.as_ref()
            && now.duration_since(snapshot.refreshed_at).as_secs() < CACHE_TTL_SECONDS
        {
            return snapshot.interfaces.clone();
        }

        let interfaces = netdev::get_interfaces();
        *cache = Some(CachedNetworkInterfaces {
            interfaces: interfaces.clone(),
            refreshed_at: now,
        });
        interfaces
    }

    pub(crate) fn shares_class_c_subnet(left: Ipv4Addr, right: Ipv4Addr) -> bool {
        let left = left.octets();
        let right = right.octets();
        left[..3] == right[..3]
    }

    fn add_class_c_broadcast(addresses: &mut Vec<Ipv4Addr>, address: Ipv4Addr) {
        let octets = address.octets();
        let broadcast = Ipv4Addr::new(octets[0], octets[1], octets[2], 255);
        if !addresses.contains(&broadcast) {
            addresses.push(broadcast);
        }
    }

    fn default_route_ipv4() -> Option<Ipv4Addr> {
        let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
        socket.connect("8.8.8.8:80").ok()?;

        match socket.local_addr().ok()? {
            SocketAddr::V4(address) if !address.ip().is_loopback() => Some(*address.ip()),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn class_c_subnet_comparison_requires_the_first_three_octets_to_match() {
        assert!(NetworkInterfaces::shares_class_c_subnet(
            Ipv4Addr::new(192, 168, 42, 1),
            Ipv4Addr::new(192, 168, 42, 254),
        ));
        assert!(!NetworkInterfaces::shares_class_c_subnet(
            Ipv4Addr::new(192, 168, 42, 1),
            Ipv4Addr::new(192, 168, 43, 1),
        ));
    }

    #[test]
    fn ipv6_address_is_not_a_usb_tether_address() {
        let address: IpAddr = "::1".parse().unwrap();

        assert!(!NetworkInterfaces::is_usb_tether_ip(&address));
    }

    #[test]
    fn known_android_usb_tether_subnets_are_detected() {
        for address in ["192.168.42.129", "172.20.10.5"] {
            let address: IpAddr = address.parse().unwrap();

            assert!(NetworkInterfaces::is_usb_tether_ip(&address));
        }
    }
}
