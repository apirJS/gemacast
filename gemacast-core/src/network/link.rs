use std::net::IpAddr;
use std::sync::Mutex;
use std::time::Instant;

use crate::domain::types::{ConnectionMode, NetworkLink};

use super::{InterfaceClassifier, NetworkInterfaces};

const WIFI_LINK_CACHE_TTL_SECONDS: u64 = 5;

static WIFI_LINK_CACHE: Mutex<Option<CachedWifiLink>> = Mutex::new(None);

struct CachedWifiLink {
    link: NetworkLink,
    measured_at: Instant,
}

pub struct NetworkLinkDetector;

impl NetworkLinkDetector {
    pub fn detect(mode: ConnectionMode, client_ip: IpAddr) -> NetworkLink {
        tracing::info!(?mode, %client_ip, "Detecting PC network link");

        match mode {
            ConnectionMode::Adb => NetworkLink::Adb,
            ConnectionMode::Usb => NetworkLink::UsbTether,
            ConnectionMode::Wifi => Self::wifi_or_ethernet_link(client_ip),
        }
    }

    pub fn from_wifi_channel(channel: u32) -> NetworkLink {
        match channel {
            0 => NetworkLink::WifiUnknown,
            1..=14 => NetworkLink::Wifi2_4Ghz,
            _ => NetworkLink::Wifi5Ghz,
        }
    }

    fn wifi_or_ethernet_link(client_ip: IpAddr) -> NetworkLink {
        let interfaces = NetworkInterfaces::all();
        let Some(interface) = Self::interface_for_client(&interfaces, client_ip) else {
            tracing::warn!("No matching interface found, PC link = Unknown");
            return NetworkLink::Unknown;
        };

        let capabilities = InterfaceClassifier::classify(interface);
        tracing::info!(
            interface = %interface.name,
            wifi = capabilities.supports_wifi(),
            "PC link: matched interface for client"
        );

        if !capabilities.supports_wifi() {
            tracing::info!(link = ?NetworkLink::Ethernet, interface = %interface.name, "PC link detected");
            return NetworkLink::Ethernet;
        }

        let link = WifiLinkCache::current();
        tracing::info!(?link, "PC link detected (connected channel query)");
        link
    }

    fn interface_for_client(
        interfaces: &[netdev::Interface],
        client_ip: IpAddr,
    ) -> Option<&netdev::Interface> {
        let matching_interface = match client_ip {
            IpAddr::V4(client_ip) => interfaces.iter().find(|interface| {
                interface.ipv4.iter().any(|network| {
                    NetworkInterfaces::shares_class_c_subnet(network.addr(), client_ip)
                })
            }),
            IpAddr::V6(_) => None,
        };

        matching_interface.or_else(|| {
            interfaces.iter().find(|interface| {
                interface
                    .ipv4
                    .iter()
                    .any(|network| !network.addr().is_loopback())
            })
        })
    }
}

struct WifiLinkCache;

impl WifiLinkCache {
    fn current() -> NetworkLink {
        let mut cache = WIFI_LINK_CACHE
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let now = Instant::now();

        if let Some(snapshot) = cache.as_ref()
            && now.duration_since(snapshot.measured_at).as_secs() < WIFI_LINK_CACHE_TTL_SECONDS
        {
            return snapshot.link;
        }

        let link = WifiChannelProbe::detect_link();
        *cache = Some(CachedWifiLink {
            link,
            measured_at: now,
        });
        link
    }
}

struct WifiChannelProbe;

impl WifiChannelProbe {
    fn detect_link() -> NetworkLink {
        #[cfg(target_os = "android")]
        {
            NetworkLink::WifiUnknown
        }

        #[cfg(target_os = "windows")]
        {
            Self::detect_windows_link()
        }

        #[cfg(target_os = "macos")]
        {
            Self::detect_macos_link()
        }

        #[cfg(target_os = "linux")]
        {
            Self::detect_linux_link()
        }

        #[cfg(not(any(
            target_os = "android",
            target_os = "windows",
            target_os = "macos",
            target_os = "linux"
        )))]
        {
            NetworkLink::WifiUnknown
        }
    }

    #[cfg(target_os = "windows")]
    fn detect_windows_link() -> NetworkLink {
        let output = crate::process::quiet_command("netsh")
            .args(["wlan", "show", "interfaces"])
            .output();

        match output {
            Ok(output) if output.status.success() => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                Self::parse_netsh_channel(&stdout)
                    .map(NetworkLinkDetector::from_wifi_channel)
                    .unwrap_or(NetworkLink::WifiUnknown)
            }
            _ => NetworkLink::WifiUnknown,
        }
    }

    #[cfg(target_os = "macos")]
    fn detect_macos_link() -> NetworkLink {
        let output = crate::process::quiet_command("system_profiler")
            .arg("SPAirPortDataType")
            .output();

        match output {
            Ok(output) if output.status.success() => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                Self::parse_system_profiler_channel(&stdout)
                    .map(NetworkLinkDetector::from_wifi_channel)
                    .unwrap_or(NetworkLink::WifiUnknown)
            }
            _ => NetworkLink::WifiUnknown,
        }
    }

    #[cfg(target_os = "linux")]
    fn detect_linux_link() -> NetworkLink {
        let iwgetid = crate::process::quiet_command("iwgetid")
            .args(["--channel", "--raw"])
            .output();

        if let Ok(output) = iwgetid
            && output.status.success()
            && let Ok(channel) = String::from_utf8_lossy(&output.stdout).trim().parse()
        {
            return NetworkLinkDetector::from_wifi_channel(channel);
        }

        let nmcli = crate::process::quiet_command("nmcli")
            .args(["-t", "-f", "IN-USE,CHAN", "dev", "wifi", "list"])
            .output();

        match nmcli {
            Ok(output) if output.status.success() => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                Self::parse_nmcli_channel(&stdout)
                    .map(NetworkLinkDetector::from_wifi_channel)
                    .unwrap_or(NetworkLink::WifiUnknown)
            }
            _ => NetworkLink::WifiUnknown,
        }
    }

    #[cfg(target_os = "windows")]
    fn parse_netsh_channel(output: &str) -> Option<u32> {
        output.lines().find_map(|line| {
            let value = line
                .trim()
                .strip_prefix("Channel")?
                .trim()
                .strip_prefix(':')?;
            value.trim().parse().ok()
        })
    }

    #[cfg(target_os = "macos")]
    fn parse_system_profiler_channel(output: &str) -> Option<u32> {
        let mut current_network = false;

        for line in output.lines() {
            let line = line.trim();
            if line.contains("Current Network Information") {
                current_network = true;
                continue;
            }
            if current_network && let Some(value) = line.strip_prefix("Channel:") {
                return value
                    .trim()
                    .split(|character: char| !character.is_ascii_digit())
                    .next()?
                    .parse()
                    .ok();
            }
        }

        None
    }

    #[cfg(target_os = "linux")]
    fn parse_nmcli_channel(output: &str) -> Option<u32> {
        output
            .lines()
            .find_map(|line| line.strip_prefix("*:")?.trim().parse().ok())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wifi_channels_zero_through_fourteen_have_the_expected_bands() {
        assert_eq!(
            NetworkLinkDetector::from_wifi_channel(0),
            NetworkLink::WifiUnknown
        );

        for channel in 1..=14 {
            assert_eq!(
                NetworkLinkDetector::from_wifi_channel(channel),
                NetworkLink::Wifi2_4Ghz
            );
        }
    }

    #[test]
    fn high_wifi_channels_are_classified_as_five_ghz_or_higher() {
        for channel in [15, 36, 149] {
            assert_eq!(
                NetworkLinkDetector::from_wifi_channel(channel),
                NetworkLink::Wifi5Ghz
            );
        }
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn netsh_channel_parser_reads_channel_values() {
        assert_eq!(
            WifiChannelProbe::parse_netsh_channel("Channel                : 36"),
            Some(36)
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn system_profiler_channel_parser_reads_the_current_network() {
        let output = "Current Network Information:\n  Channel: 149 (5GHz, 80MHz)";

        assert_eq!(
            WifiChannelProbe::parse_system_profiler_channel(output),
            Some(149)
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn nmcli_channel_parser_reads_the_active_network() {
        assert_eq!(WifiChannelProbe::parse_nmcli_channel("*:36"), Some(36));
    }
}
