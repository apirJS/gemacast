pub mod discovery;
pub mod receiver;
pub mod sender;

pub use discovery::{
    DiscoveryBroadcaster, DiscoveryBroadcasterHandles, DiscoveryListener, DiscoveryListenerHandles,
    send_control_message,
};
pub use receiver::{AudioReceiver, AudioReceiverHandles};
pub use sender::{AudioSender, SenderCommand};

pub const DISCOVERY_PORT: u16 = 55555;
pub const AUDIO_PORT: u16 = 55556;

/// The specific buffer size (in samples) handed to CPAL.
/// 480 samples = 10ms at 48kHz. Set to a lower number for faster audio loop triggers.
pub const CPAL_BUFFER_SIZE: u32 = 512;

pub fn get_local_ip() -> Result<std::net::IpAddr, String> {
    let iface = netdev::get_default_interface().map_err(|e| e.to_string())?;
    if let Some(ip) = iface.ipv4.first() {
        Ok(std::net::IpAddr::V4(ip.addr()))
    } else if let Some(ip) = iface.ipv6.first() {
        Ok(std::net::IpAddr::V6(ip.addr()))
    } else {
        Err("No IPs assigned to default interface".to_string())
    }
}

pub fn get_broadcast_addrs() -> Vec<std::net::Ipv4Addr> {
    let mut addrs = Vec::new();

    let interfaces = netdev::get_interfaces();
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

pub fn is_usb_tether_ip(ip: &std::net::IpAddr) -> bool {
    if let std::net::IpAddr::V4(ipv4) = ip {
        let octets = ipv4.octets();

        if octets[0] == 192 && octets[1] == 168 && (octets[2] == 42 || octets[2] == 43) {
            return true;
        }

        let interfaces = netdev::get_interfaces();
        for interface in interfaces {
            let mut name_lower = interface.name.to_lowercase();
            if let Some(ref friendly) = interface.friendly_name {
                name_lower.push_str(" ");
                name_lower.push_str(&friendly.to_lowercase());
            }
            if let Some(ref desc) = interface.description {
                name_lower.push_str(" ");
                name_lower.push_str(&desc.to_lowercase());
            }

            if name_lower.contains("rndis")
                || name_lower.contains("ndis")
                || (!name_lower.contains("wlan")
                    && !name_lower.contains("wi-fi")
                    && !name_lower.contains("wifi")
                    && !name_lower.contains("wireless")
                    && !name_lower.contains("lo")
                    && !name_lower.contains("swlan")
                    && !name_lower.contains("p2p")
                    && !name_lower.contains("dummy")
                    && !name_lower.contains("tun"))
            {
                for ip_net in interface.ipv4 {
                    let local_octets = ip_net.addr().octets();
                    if octets[0] == local_octets[0]
                        && octets[1] == local_octets[1]
                        && octets[2] == local_octets[2]
                    {
                        return true;
                    }
                }
            }
        }
    }
    false
}
