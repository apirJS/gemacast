use std::sync::Mutex;
use std::time::Instant;

struct CachedInterfaces {
    interfaces: Vec<netdev::Interface>,
    last_refresh: Instant,
}

static INTERFACE_CACHE: Mutex<Option<CachedInterfaces>> = Mutex::new(None);
const CACHE_TTL_SECS: u64 = 5;

fn cached_interfaces() -> Vec<netdev::Interface> {
    let mut guard = INTERFACE_CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let now = Instant::now();
    if let Some(ref cache) = *guard
        && now.duration_since(cache.last_refresh).as_secs() < CACHE_TTL_SECS
    {
        return cache.interfaces.clone();
    }
    let interfaces = netdev::get_interfaces();
    *guard = Some(CachedInterfaces {
        interfaces: interfaces.clone(),
        last_refresh: now,
    });
    interfaces
}

pub fn get_local_ip() -> Result<std::net::IpAddr, String> {
    for iface in cached_interfaces() {
        for ip_net in &iface.ipv4 {
            if !ip_net.addr().is_loopback() {
                return Ok(std::net::IpAddr::V4(ip_net.addr()));
            }
        }
    }
    for iface in cached_interfaces() {
        for ip_net in &iface.ipv6 {
            if !ip_net.addr().is_loopback() {
                return Ok(std::net::IpAddr::V6(ip_net.addr()));
            }
        }
    }
    Err("No non-loopback IPs found on any interface".to_string())
}

pub fn get_broadcast_addrs() -> Vec<std::net::Ipv4Addr> {
    let mut addrs = Vec::new();
    let interfaces = cached_interfaces();
    for interface in interfaces {
        for ip_net in interface.ipv4 {
            let ipv4 = ip_net.addr();
            if !ipv4.is_loopback() {
                let octets = ipv4.octets();
                let bcast = std::net::Ipv4Addr::new(octets[0], octets[1], octets[2], 255);
                if !addrs.contains(&bcast) {
                    addrs.push(bcast);
                }
            }
        }
    }
    if let Ok(socket) = std::net::UdpSocket::bind("0.0.0.0:0")
        && socket.connect("8.8.8.8:80").is_ok()
        && let Ok(std::net::SocketAddr::V4(addr)) = socket.local_addr()
    {
        let ipv4 = addr.ip();
        if !ipv4.is_loopback() {
            let octets = ipv4.octets();
            let bcast = std::net::Ipv4Addr::new(octets[0], octets[1], octets[2], 255);
            if !addrs.contains(&bcast) {
                addrs.push(bcast);
            }
        }
    }
    if addrs.is_empty() {
        addrs.push(std::net::Ipv4Addr::BROADCAST);
    }
    addrs
}

/// Android's tethering gateways are fixed per transport, which makes the
/// interface's own address a stronger signal than its name.
///
/// | transport | gateway |
/// | --- | --- |
/// | USB / RNDIS | `192.168.42.1` |
/// | Wi-Fi hotspot (soft AP) | `192.168.43.1` |
/// | Bluetooth | `192.168.44.1` |
///
/// Android 11+ may randomise the hotspot subnet, so the name heuristics below
/// remain the backstop.
fn is_soft_ap_subnet(ip: &std::net::Ipv4Addr) -> bool {
    let octets = ip.octets();
    octets[0] == 192 && octets[1] == 168 && octets[2] == 43
}

pub fn classify_interface(interface: &netdev::Interface) -> (bool, bool) {
    let if_type = &interface.if_type;
    let name_lower = interface.name.to_lowercase();
    let desc_lower = interface
        .description
        .as_deref()
        .unwrap_or("")
        .to_lowercase();
    let friendly_lower = interface
        .friendly_name
        .as_deref()
        .unwrap_or("")
        .to_lowercase();

    // 1. Cellular check
    if *if_type == netdev::prelude::InterfaceType::Wwanpp
        || *if_type == netdev::prelude::InterfaceType::Wwanpp2
    {
        return (false, false);
    }
    let is_cellular = name_lower.contains("rmnet")
        || name_lower.contains("ccmni")
        || name_lower.contains("ppp")
        || name_lower.contains("pdp")
        || name_lower.contains("wwan")
        || name_lower.contains("gnss")
        || name_lower.contains("rmnet_data");

    if is_cellular {
        return (false, false);
    }

    // 2. WiFi check (100% native via OS)
    //
    // A phone acting as a Wi-Fi hotspot is the load-bearing case here. Its
    // soft-AP interface is named `ap0` / `softap0` / `swlan0` depending on the
    // vendor — none of which contain "wlan" — so before this check existed it
    // failed the Wi-Fi test and fell into the "assume USB cable" fallback below.
    // That made a hotspot indistinguishable from a real cable: `modes.usb` went
    // true while `modes.wifi` stayed false (so the Wi-Fi button was disabled),
    // and the link reported `UsbTether`, which selects a jitter profile capped
    // at 100 ms — sized for USB's ~22 ms delivery grid, and unable to cover the
    // 100-600 ms gaps a radio link actually produces.
    let has_soft_ap_addr = interface
        .ipv4
        .iter()
        .any(|net| is_soft_ap_subnet(&net.addr()));
    let is_soft_ap_name = name_lower.starts_with("ap")
        || name_lower.contains("softap")
        || name_lower.contains("swlan")
        || name_lower.contains("p2p");

    let is_wifi = *if_type == netdev::prelude::InterfaceType::Wireless80211
        // Fallback for Android/Linux where if_type might just be Ethernet or Unknown
        || name_lower.contains("wlan")
        || name_lower.contains("wifi")
        || has_soft_ap_addr
        || is_soft_ap_name;

    // 3. USB Tethering check (NDIS / RNDIS)
    // Windows/macOS reports USB tethering as Ethernet, so we check descriptions for "ndis".
    // Android reports it as "rndis0".
    let is_ndis = name_lower.contains("rndis")
        || name_lower.contains("ndis")
        || desc_lower.contains("ndis")
        || friendly_lower.contains("ndis");

    let is_usb = is_ndis
        // Positive evidence, checked before the fallback so a rename cannot lose it.
        || name_lower.starts_with("usb")
        || interface.ipv4.iter().any(|net| is_usb_subnet(&net.addr()))
        // Aggressive fallback for Android: if it's not WiFi/Loopback/VPN, assume it's the USB cable.
        // We restrict this aggressive fallback slightly by checking it's not a known PC Ethernet.
        || (!is_wifi
            && *if_type != netdev::prelude::InterfaceType::Loopback
            && !name_lower.contains("lo")
            && !name_lower.contains("dummy")
            && !name_lower.contains("tun")
            && !desc_lower.contains("pcie")
            && !desc_lower.contains("gigabit")
            && !desc_lower.contains("ethernet")); // basic guards to prevent PC physical ethernet from matching

    (is_wifi, is_usb)
}

fn is_usb_subnet(ip: &std::net::Ipv4Addr) -> bool {
    let octets = ip.octets();
    (octets[0] == 192 && octets[1] == 168 && (octets[2] == 42 || octets[2] == 45))
        || (octets[0] == 172 && octets[1] == 20 && octets[2] == 10)
}

pub fn is_usb_tether_ip(ip: &std::net::IpAddr) -> bool {
    let std::net::IpAddr::V4(ipv4) = ip else {
        return false;
    };
    if is_usb_subnet(ipv4) {
        return true;
    }

    cached_interfaces().iter().any(|iface| {
        let (_, is_usb) = classify_interface(iface);
        is_usb
            && iface.ipv4.iter().any(|net| {
                let local = net.addr().octets();
                let target = ipv4.octets();
                local[0] == target[0] && local[1] == target[1] && local[2] == target[2]
            })
    })
}
use crate::domain::types::{ConnectionMode, NetworkLink};
/// Classify a WiFi channel number into a band.
///
/// - Channels 1–14 → 2.4 GHz
/// - Channels 32+ → 5 GHz / 6 GHz
/// - Channel 0 → unknown
pub fn classify_channel(channel: u32) -> NetworkLink {
    if channel == 0 {
        NetworkLink::WifiUnknown
    } else if channel <= 14 {
        NetworkLink::Wifi2_4Ghz
    } else {
        NetworkLink::Wifi5Ghz
    }
}

/// Detect the PC's network link type for a specific connection.
///
/// Strategy — use the phone's `client_ip` (from the TCP connection) to
/// determine the transport without guessing:
///
/// 1. Loopback IP (127.x) → ADB port-forwarding.
/// 2. Known USB-tether subnet → USB tether.
/// 4. If WiFi, query the OS for the connected channel to derive the band.
/// 5. Fallback: [`NetworkLink::WifiUnknown`] if query fails.
pub fn detect_pc_link(mode: ConnectionMode, client_ip: std::net::IpAddr) -> NetworkLink {
    tracing::info!(?mode, %client_ip, "Detecting PC network link");

    // 1. ADB mode → Always ADB
    if mode == ConnectionMode::Adb {
        tracing::info!(link = ?NetworkLink::Adb, "PC link detected (ADB mode)");
        return NetworkLink::Adb;
    }

    // 2. USB mode → Always USB Tether
    if mode == ConnectionMode::Usb {
        tracing::info!(link = ?NetworkLink::UsbTether, "PC link detected (USB mode)");
        return NetworkLink::UsbTether;
    }

    // 3. WIFI mode → Find the local interface whose /24 subnet contains the client IP
    let interfaces = cached_interfaces();
    let primary = if let std::net::IpAddr::V4(client_v4) = client_ip {
        let ct = client_v4.octets();
        interfaces.iter().find(|iface| {
            iface.ipv4.iter().any(|net| {
                let lo = net.addr().octets();
                lo[0] == ct[0] && lo[1] == ct[1] && lo[2] == ct[2]
            })
        })
    } else {
        None
    }
    // Fallback: first non-loopback interface
    .or_else(|| {
        interfaces
            .iter()
            .find(|iface| iface.ipv4.iter().any(|net| !net.addr().is_loopback()))
    });

    let Some(primary) = primary else {
        tracing::warn!("No matching interface found, PC link = Unknown");
        return NetworkLink::Unknown;
    };

    let (is_wifi, _is_usb) = classify_interface(primary);
    tracing::info!(
        iface = %primary.name,
        is_wifi,
        "PC link: matched interface for client"
    );

    if !is_wifi {
        tracing::info!(link = ?NetworkLink::Ethernet, iface = %primary.name, "PC link detected");
        return NetworkLink::Ethernet;
    }

    // Query the OS for the connected Wi-Fi channel instantly (no hardware scan).
    let link = get_connected_wifi_channel();
    tracing::info!(?link, "PC link detected (connected channel query)");
    link
}

/// Query the OS for the channel the Wi-Fi interface is currently connected to.
///
/// Uses platform-specific CLI commands that return instantly (<50ms),
/// as opposed to hardware scans which block for 3-5 seconds.
fn get_connected_wifi_channel() -> NetworkLink {
    #[cfg(target_os = "android")]
    return NetworkLink::WifiUnknown;

    #[cfg(target_os = "windows")]
    {
        // `netsh wlan show interfaces` outputs key-value lines like:
        //     Channel                : 36
        let output = crate::process::quiet_command("netsh")
            .args(["wlan", "show", "interfaces"])
            .output();

        match output {
            Ok(out) if out.status.success() => {
                let stdout = String::from_utf8_lossy(&out.stdout);
                if let Some(channel) = parse_netsh_channel(&stdout) {
                    let link = classify_channel(channel);
                    tracing::info!(channel, ?link, "Connected Wi-Fi channel (netsh)");
                    link
                } else {
                    tracing::warn!("Could not parse channel from netsh output");
                    NetworkLink::WifiUnknown
                }
            }
            Ok(out) => {
                tracing::warn!(code = ?out.status.code(), "netsh exited with error");
                NetworkLink::WifiUnknown
            }
            Err(e) => {
                tracing::warn!("Failed to run netsh: {e}");
                NetworkLink::WifiUnknown
            }
        }
    }

    #[cfg(target_os = "macos")]
    {
        // `system_profiler SPAirPortDataType` outputs lines like:
        //     Channel: 149 (5GHz, 80MHz)
        let output = crate::process::quiet_command("system_profiler")
            .arg("SPAirPortDataType")
            .output();

        match output {
            Ok(out) if out.status.success() => {
                let stdout = String::from_utf8_lossy(&out.stdout);
                if let Some(channel) = parse_system_profiler_channel(&stdout) {
                    let link = classify_channel(channel);
                    tracing::info!(channel, ?link, "Connected Wi-Fi channel (system_profiler)");
                    link
                } else {
                    tracing::warn!("Could not parse channel from system_profiler output");
                    NetworkLink::WifiUnknown
                }
            }
            Ok(out) => {
                tracing::warn!(code = ?out.status.code(), "system_profiler exited with error");
                NetworkLink::WifiUnknown
            }
            Err(e) => {
                tracing::warn!("Failed to run system_profiler: {e}");
                NetworkLink::WifiUnknown
            }
        }
    }

    #[cfg(target_os = "linux")]
    {
        // Try `iwgetid --channel --raw` first (outputs bare channel number like "6").
        // Falls back to `nmcli` if iwgetid is unavailable.
        let output = crate::process::quiet_command("iwgetid")
            .args(["--channel", "--raw"])
            .output();

        if let Ok(out) = output
            && out.status.success()
        {
            let stdout = String::from_utf8_lossy(&out.stdout);
            if let Ok(channel) = stdout.trim().parse::<u32>() {
                let link = classify_channel(channel);
                tracing::info!(channel, ?link, "Connected Wi-Fi channel (iwgetid)");
                return link;
            }
        }

        // Fallback: nmcli -t -f IN-USE,CHAN dev wifi list
        // Connected network line starts with "*:", e.g. "*:36"
        let output = crate::process::quiet_command("nmcli")
            .args(["-t", "-f", "IN-USE,CHAN", "dev", "wifi", "list"])
            .output();

        match output {
            Ok(out) if out.status.success() => {
                let stdout = String::from_utf8_lossy(&out.stdout);
                if let Some(channel) = parse_nmcli_channel(&stdout) {
                    let link = classify_channel(channel);
                    tracing::info!(channel, ?link, "Connected Wi-Fi channel (nmcli)");
                    link
                } else {
                    tracing::warn!("Could not find connected network in nmcli output");
                    NetworkLink::WifiUnknown
                }
            }
            _ => {
                tracing::warn!("Both iwgetid and nmcli failed, cannot determine Wi-Fi channel");
                NetworkLink::WifiUnknown
            }
        }
    }
}

/// Parse `Channel : 36` from `netsh wlan show interfaces` output.
#[cfg(target_os = "windows")]
fn parse_netsh_channel(output: &str) -> Option<u32> {
    for line in output.lines() {
        let trimmed = line.trim();
        // Match lines like "Channel                : 36"
        if let Some(rest) = trimmed.strip_prefix("Channel")
            && let Some(value) = rest.trim().strip_prefix(':')
        {
            return value.trim().parse().ok();
        }
    }
    None
}

/// Parse `Channel: 149 (5GHz, 80MHz)` from `system_profiler SPAirPortDataType` output.
/// Only looks in the "Current Network Information" section.
#[cfg(target_os = "macos")]
fn parse_system_profiler_channel(output: &str) -> Option<u32> {
    let mut in_current_network = false;
    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.contains("Current Network Information") {
            in_current_network = true;
            continue;
        }
        if in_current_network && let Some(rest) = trimmed.strip_prefix("Channel:") {
            // "149 (5GHz, 80MHz)" → take the number before the space/paren
            let channel_str = rest.trim().split(|c: char| !c.is_ascii_digit()).next()?;
            return channel_str.parse().ok();
        }
    }
    None
}

/// Parse the connected network channel from `nmcli -t -f IN-USE,CHAN dev wifi list`.
/// Connected line looks like `*:36`.
#[cfg(target_os = "linux")]
fn parse_nmcli_channel(output: &str) -> Option<u32> {
    for line in output.lines() {
        if let Some(rest) = line.strip_prefix("*:") {
            return rest.trim().parse().ok();
        }
    }
    None
}

#[cfg(test)]
mod tests {
    fn make_interface(
        name: &str,
        if_type: netdev::prelude::InterfaceType,
        description: Option<&str>,
        friendly_name: Option<&str>,
    ) -> netdev::Interface {
        let mut iface = netdev::Interface::dummy();
        iface.name = name.to_string();
        iface.if_type = if_type;
        iface.description = description.map(|s| s.to_string());
        iface.friendly_name = friendly_name.map(|s| s.to_string());
        iface
    }

    /// Same as [`make_interface`] but with one IPv4 address attached, so the
    /// subnet-based branches of `classify_interface` are reachable.
    fn make_interface_with_ip(
        name: &str,
        if_type: netdev::prelude::InterfaceType,
        ip: &str,
    ) -> netdev::Interface {
        let mut iface = make_interface(name, if_type, None, None);
        let addr: std::net::Ipv4Addr = ip.parse().unwrap();
        iface.ipv4 = vec![netdev::ipnet::Ipv4Net::new(addr, 24).unwrap()];
        iface
    }

    mod classify_interface {
        use super::*;

        #[test]
        fn should_identify_native_wireless_as_wifi() {
            let iface = make_interface(
                "{WIFI-UUID}",
                netdev::prelude::InterfaceType::Wireless80211,
                Some("Intel Wi-Fi 6 AX200"),
                Some("Wi-Fi"),
            );
            let (is_wifi, is_usb) = super::super::classify_interface(&iface);
            assert!(
                is_wifi,
                "Expected Wireless80211 to be classified as wifi natively"
            );
            assert!(!is_usb, "Wireless80211 should not be classified as usb");
        }

        #[test]
        fn should_identify_fallback_wlan0_as_wifi() {
            // Testing fallback for Android where if_type might just be unknown/ethernet
            let iface = make_interface(
                "wlan0",
                netdev::prelude::InterfaceType::Ethernet,
                None,
                None,
            );
            let (is_wifi, is_usb) = super::super::classify_interface(&iface);
            assert!(is_wifi, "Expected wlan0 fallback to be classified as wifi");
            assert!(!is_usb, "wlan0 should not be classified as usb");
        }

        #[test]
        fn should_identify_windows_rndis_as_usb() {
            // Windows reports USB tether as Ethernet but sets description to include NDIS
            let iface = make_interface(
                "{ETH-UUID}",
                netdev::prelude::InterfaceType::Ethernet,
                Some("Remote NDIS based Internet Sharing Device"),
                Some("Ethernet 2"),
            );
            let (is_wifi, is_usb) = super::super::classify_interface(&iface);
            assert!(!is_wifi, "RNDIS should not be classified as wifi");
            assert!(is_usb, "Expected NDIS description to be classified as usb");
        }

        #[test]
        fn should_identify_android_rndis0_as_usb() {
            let iface = make_interface(
                "rndis0",
                netdev::prelude::InterfaceType::Unknown,
                None,
                None,
            );
            let (is_wifi, is_usb) = super::super::classify_interface(&iface);
            assert!(!is_wifi, "rndis0 should not be classified as wifi");
            assert!(is_usb, "Expected rndis0 to be classified as usb");
        }

        #[test]
        fn should_identify_wwan_as_cellular_returning_both_false() {
            let iface = make_interface(
                "{WWAN-UUID}",
                netdev::prelude::InterfaceType::Wwanpp,
                Some("Generic Mobile Broadband Adapter"),
                Some("Cellular"),
            );
            let (is_wifi, is_usb) = super::super::classify_interface(&iface);
            assert!(!is_wifi, "WWAN should not be wifi");
            assert!(!is_usb, "WWAN should not be usb");
        }

        #[test]
        fn should_not_classify_standard_ethernet_as_usb() {
            let iface = make_interface(
                "{ETH-UUID}",
                netdev::prelude::InterfaceType::Ethernet,
                Some("Realtek PCIe GbE Family Controller"),
                Some("Ethernet"),
            );
            let (is_wifi, is_usb) = super::super::classify_interface(&iface);
            assert!(!is_wifi, "Standard ethernet is not wifi");
            assert!(!is_usb, "Standard ethernet is not usb");
        }

        // A phone sharing its connection as a Wi-Fi hotspot must not look like a
        // cable. Before these, the soft-AP interface fell through to the
        // "assume USB" fallback, which disabled the Wi-Fi mode button and picked
        // a 100 ms-capped jitter profile for a radio link.
        #[test]
        fn should_identify_soft_ap_subnet_as_wifi_not_usb() {
            // 192.168.43.1 is Android's Wi-Fi-hotspot gateway; .42 is USB.
            let iface = make_interface_with_ip(
                "ap0",
                netdev::prelude::InterfaceType::Ethernet,
                "192.168.43.1",
            );
            let (is_wifi, is_usb) = super::super::classify_interface(&iface);
            assert!(is_wifi, "Soft-AP subnet should be classified as wifi");
            assert!(!is_usb, "Soft-AP must not be classified as usb");
        }

        #[test]
        fn should_identify_vendor_soft_ap_names_as_wifi() {
            for name in ["ap0", "softap0", "swlan0", "p2p0"] {
                let iface =
                    make_interface(name, netdev::prelude::InterfaceType::Unknown, None, None);
                let (is_wifi, is_usb) = super::super::classify_interface(&iface);
                assert!(is_wifi, "{name} should be classified as wifi");
                assert!(!is_usb, "{name} must not be classified as usb");
            }
        }

        #[test]
        fn should_still_identify_usb_tether_subnet_as_usb() {
            // The other half of the discriminator: 192.168.42.x stays USB, so
            // fixing the hotspot case cannot regress real tethering.
            let iface = make_interface_with_ip(
                "rndis0",
                netdev::prelude::InterfaceType::Ethernet,
                "192.168.42.129",
            );
            let (is_wifi, is_usb) = super::super::classify_interface(&iface);
            assert!(!is_wifi, "USB tether subnet is not wifi");
            assert!(is_usb, "USB tether subnet should be classified as usb");
        }

        #[test]
        fn should_identify_usb0_by_name_as_usb() {
            let iface = make_interface("usb0", netdev::prelude::InterfaceType::Unknown, None, None);
            let (is_wifi, is_usb) = super::super::classify_interface(&iface);
            assert!(!is_wifi, "usb0 is not wifi");
            assert!(is_usb, "usb0 should be classified as usb");
        }

        #[test]
        fn should_keep_wlan_as_wifi_when_holding_a_hotspot_client_address() {
            // The PC side of the same link: its own Wi-Fi adapter is a *client*
            // of the phone's AP and sits on 192.168.43.x too. It must stay Wi-Fi.
            let iface = make_interface_with_ip(
                "wlan0",
                netdev::prelude::InterfaceType::Wireless80211,
                "192.168.43.55",
            );
            let (is_wifi, is_usb) = super::super::classify_interface(&iface);
            assert!(is_wifi, "PC Wi-Fi client of a hotspot is still wifi");
            assert!(!is_usb, "PC Wi-Fi client of a hotspot is not usb");
        }
    }

    mod is_usb_tether_ip {
        #[test]
        fn should_return_false_for_ipv6() {
            let ip: std::net::IpAddr = "::1".parse().unwrap();
            assert!(
                !super::super::is_usb_tether_ip(&ip),
                "IPv6 addresses should never be USB tether"
            );
        }

        #[test]
        fn should_return_true_for_known_usb_subnet_192_168_42() {
            let ip: std::net::IpAddr = "192.168.42.129".parse().unwrap();
            assert!(
                super::super::is_usb_tether_ip(&ip),
                "192.168.42.x is a known USB tether subnet"
            );
        }

        #[test]
        fn should_return_true_for_known_usb_subnet_172_20_10() {
            let ip: std::net::IpAddr = "172.20.10.5".parse().unwrap();
            assert!(
                super::super::is_usb_tether_ip(&ip),
                "172.20.10.x is a known USB tether subnet"
            );
        }
    }

    mod classify_channel {
        use super::super::classify_channel;
        use crate::domain::types::NetworkLink;

        #[test]
        fn channel_0_should_be_unknown() {
            assert_eq!(classify_channel(0), NetworkLink::WifiUnknown);
        }

        #[test]
        fn channels_1_to_14_should_be_2_4ghz() {
            for ch in 1..=14 {
                assert_eq!(
                    classify_channel(ch),
                    NetworkLink::Wifi2_4Ghz,
                    "Channel {ch} should be 2.4GHz"
                );
            }
        }

        #[test]
        fn channel_36_should_be_5ghz() {
            assert_eq!(classify_channel(36), NetworkLink::Wifi5Ghz);
        }

        #[test]
        fn channel_149_should_be_5ghz() {
            assert_eq!(classify_channel(149), NetworkLink::Wifi5Ghz);
        }

        #[test]
        fn channel_15_should_be_5ghz() {
            // Channel 15 is technically unused but falls into 5GHz by our rule
            assert_eq!(classify_channel(15), NetworkLink::Wifi5Ghz);
        }
    }
}
