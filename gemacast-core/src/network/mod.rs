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

fn classify_interface(interface: &netdev::Interface) -> (bool, bool) {
    let name_lower = interface.name.to_lowercase();
    
    // Explicitly ignore cellular/modem/system interfaces
    let is_cellular = name_lower.contains("rmnet") || 
                      name_lower.contains("ccmni") || 
                      name_lower.contains("ppp") || 
                      name_lower.contains("pdp") ||
                      name_lower.contains("wwan") ||
                      name_lower.contains("gnss") ||
                      name_lower.contains("rmnet_data");
    
    if is_cellular {
        return (false, false);
    }

    let is_wifi = name_lower.contains("wlan") || name_lower.contains("wifi");
    let is_usb = name_lower.contains("rndis")
                || name_lower.contains("ndis")
                || (!name_lower.contains("wlan")
                    && !name_lower.contains("wi-fi")
                    && !name_lower.contains("wifi")
                    && !name_lower.contains("wireless")
                    && !name_lower.contains("lo")
                    && !name_lower.contains("swlan")
                    && !name_lower.contains("p2p")
                    && !name_lower.contains("dummy")
                    && !name_lower.contains("tun"));

    (is_wifi, is_usb)
}

fn is_usb_subnet(ip: &std::net::Ipv4Addr) -> bool {
    let octets = ip.octets();
    (octets[0] == 192 && octets[1] == 168 && (octets[2] == 42 || octets[2] == 45)) || 
    (octets[0] == 172 && octets[1] == 20 && octets[2] == 10)
}

pub fn is_usb_tether_ip(ip: &std::net::IpAddr) -> bool {
    let std::net::IpAddr::V4(ipv4) = ip else { return false; };
    if is_usb_subnet(ipv4) { return true; }

    netdev::get_interfaces().iter().any(|iface| {
        let (_, is_usb) = classify_interface(iface);
        is_usb && iface.ipv4.iter().any(|net| {
            let local = net.addr().octets();
            let target = ipv4.octets();
            local[0] == target[0] && local[1] == target[1] && local[2] == target[2]
        })
    })
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ConnectionModes {
    pub wifi: bool,
    pub usb: bool,
}

pub fn get_available_connection_modes() -> ConnectionModes {
    let mut modes = ConnectionModes { wifi: false, usb: false };

    for interface in netdev::get_interfaces() {
        let (is_wifi, is_usb) = classify_interface(&interface);
        for ip_net in interface.ipv4 {
            if ip_net.addr().is_loopback() { continue; }
            if is_usb || is_usb_subnet(&ip_net.addr()) {
                modes.usb = true;
            } else if is_wifi {
                modes.wifi = true;
            }
        }
    }
    modes
}
