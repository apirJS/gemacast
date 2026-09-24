pub struct InterfaceClassifier;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InterfaceCapabilities {
    wifi: bool,
    usb_tethering: bool,
}

impl InterfaceCapabilities {
    pub fn supports_wifi(self) -> bool {
        self.wifi
    }

    pub fn supports_usb_tethering(self) -> bool {
        self.usb_tethering
    }
}

impl InterfaceClassifier {
    pub fn classify(interface: &netdev::Interface) -> InterfaceCapabilities {
        let interface_type = interface.if_type;
        let name = interface.name.to_lowercase();
        let description = interface
            .description
            .as_deref()
            .unwrap_or("")
            .to_lowercase();
        let friendly_name = interface
            .friendly_name
            .as_deref()
            .unwrap_or("")
            .to_lowercase();

        if Self::is_cellular(interface_type, &name) {
            return InterfaceCapabilities {
                wifi: false,
                usb_tethering: false,
            };
        }

        let wifi = Self::is_wifi(interface, &name);
        let usb_tethering =
            Self::is_usb_tether(interface, &name, &description, &friendly_name, wifi);

        InterfaceCapabilities {
            wifi,
            usb_tethering,
        }
    }

    pub(crate) fn is_usb_tether_address(ip: &std::net::Ipv4Addr) -> bool {
        let octets = ip.octets();
        (octets[0] == 192 && octets[1] == 168 && (octets[2] == 42 || octets[2] == 45))
            || (octets[0] == 172 && octets[1] == 20 && octets[2] == 10)
    }

    fn is_cellular(interface_type: netdev::prelude::InterfaceType, name: &str) -> bool {
        interface_type == netdev::prelude::InterfaceType::Wwanpp
            || interface_type == netdev::prelude::InterfaceType::Wwanpp2
            || ["rmnet", "ccmni", "ppp", "pdp", "wwan", "gnss", "rmnet_data"]
                .iter()
                .any(|marker| name.contains(marker))
    }

    fn is_wifi(interface: &netdev::Interface, name: &str) -> bool {
        interface.if_type == netdev::prelude::InterfaceType::Wireless80211
            || name.contains("wlan")
            || name.contains("wifi")
            || interface
                .ipv4
                .iter()
                .any(|network| Self::is_soft_ap_address(&network.addr()))
            || name.starts_with("ap")
            || ["softap", "swlan", "p2p"]
                .iter()
                .any(|marker| name.contains(marker))
    }

    fn is_usb_tether(
        interface: &netdev::Interface,
        name: &str,
        description: &str,
        friendly_name: &str,
        wifi: bool,
    ) -> bool {
        let has_ndis_name = name.contains("rndis") || name.contains("ndis");
        let has_ndis_description = description.contains("ndis") || friendly_name.contains("ndis");
        let has_usb_name = name.starts_with("usb");
        let has_usb_address = interface
            .ipv4
            .iter()
            .any(|network| Self::is_usb_tether_address(&network.addr()));

        has_ndis_name
            || has_ndis_description
            || has_usb_name
            || has_usb_address
            || Self::is_unidentified_mobile_interface(interface, name, description, wifi)
    }

    fn is_unidentified_mobile_interface(
        interface: &netdev::Interface,
        name: &str,
        description: &str,
        wifi: bool,
    ) -> bool {
        !wifi
            && interface.if_type != netdev::prelude::InterfaceType::Loopback
            && !name.contains("lo")
            && !name.contains("dummy")
            && !name.contains("tun")
            && !description.contains("pcie")
            && !description.contains("gigabit")
            && !description.contains("ethernet")
    }

    fn is_soft_ap_address(ip: &std::net::Ipv4Addr) -> bool {
        let octets = ip.octets();
        octets[0] == 192 && octets[1] == 168 && octets[2] == 43
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn interface(
        name: &str,
        interface_type: netdev::prelude::InterfaceType,
        description: Option<&str>,
        friendly_name: Option<&str>,
    ) -> netdev::Interface {
        let mut interface = netdev::Interface::dummy();
        interface.name = name.to_string();
        interface.if_type = interface_type;
        interface.description = description.map(str::to_string);
        interface.friendly_name = friendly_name.map(str::to_string);
        interface
    }

    fn interface_with_ipv4(
        name: &str,
        interface_type: netdev::prelude::InterfaceType,
        address: &str,
    ) -> netdev::Interface {
        let mut interface = interface(name, interface_type, None, None);
        let address = address.parse().unwrap();
        interface.ipv4 = vec![netdev::ipnet::Ipv4Net::new(address, 24).unwrap()];
        interface
    }

    #[test]
    fn native_wireless_interface_supports_wifi_without_usb_tethering() {
        let interface = interface(
            "{WIFI-UUID}",
            netdev::prelude::InterfaceType::Wireless80211,
            Some("Intel Wi-Fi 6 AX200"),
            Some("Wi-Fi"),
        );

        let capabilities = InterfaceClassifier::classify(&interface);

        assert!(capabilities.supports_wifi());
        assert!(!capabilities.supports_usb_tethering());
    }

    #[test]
    fn wlan_name_supports_wifi_without_usb_tethering() {
        let interface = interface(
            "wlan0",
            netdev::prelude::InterfaceType::Ethernet,
            None,
            None,
        );

        let capabilities = InterfaceClassifier::classify(&interface);

        assert!(capabilities.supports_wifi());
        assert!(!capabilities.supports_usb_tethering());
    }

    #[test]
    fn ndis_interface_supports_usb_tethering() {
        let interface = interface(
            "{ETH-UUID}",
            netdev::prelude::InterfaceType::Ethernet,
            Some("Remote NDIS based Internet Sharing Device"),
            Some("Ethernet 2"),
        );

        let capabilities = InterfaceClassifier::classify(&interface);

        assert!(!capabilities.supports_wifi());
        assert!(capabilities.supports_usb_tethering());
    }

    #[test]
    fn cellular_interface_supports_neither_transport() {
        let interface = interface(
            "{WWAN-UUID}",
            netdev::prelude::InterfaceType::Wwanpp,
            Some("Generic Mobile Broadband Adapter"),
            Some("Cellular"),
        );

        let capabilities = InterfaceClassifier::classify(&interface);

        assert!(!capabilities.supports_wifi());
        assert!(!capabilities.supports_usb_tethering());
    }

    #[test]
    fn standard_ethernet_interface_does_not_support_usb_tethering() {
        let interface = interface(
            "{ETH-UUID}",
            netdev::prelude::InterfaceType::Ethernet,
            Some("Realtek PCIe GbE Family Controller"),
            Some("Ethernet"),
        );

        let capabilities = InterfaceClassifier::classify(&interface);

        assert!(!capabilities.supports_wifi());
        assert!(!capabilities.supports_usb_tethering());
    }

    #[test]
    fn soft_ap_interface_supports_wifi_without_usb_tethering() {
        let interface = interface_with_ipv4(
            "ap0",
            netdev::prelude::InterfaceType::Ethernet,
            "192.168.43.1",
        );

        let capabilities = InterfaceClassifier::classify(&interface);

        assert!(capabilities.supports_wifi());
        assert!(!capabilities.supports_usb_tethering());
    }

    #[test]
    fn usb_tether_address_supports_usb_tethering() {
        let interface = interface_with_ipv4(
            "rndis0",
            netdev::prelude::InterfaceType::Ethernet,
            "192.168.42.129",
        );

        let capabilities = InterfaceClassifier::classify(&interface);

        assert!(!capabilities.supports_wifi());
        assert!(capabilities.supports_usb_tethering());
    }
}
