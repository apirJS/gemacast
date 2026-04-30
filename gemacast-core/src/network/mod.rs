pub mod adb;
pub mod discovery;
pub mod receiver;
pub mod sender;
pub mod transport;

pub use discovery::{DiscoveryBroadcaster, DiscoveryListener, send_control_message};
pub use receiver::AudioReceiver;
pub use sender::{AudioSender, SenderCommand};

pub use crate::types::{ConnectionModes, get_available_connection_modes};

/// Well-known port constants used across the GemaCast protocol.
pub struct Ports;

impl Ports {
    /// UDP port for discovery broadcasts and control messages.
    pub const DISCOVERY: u16 = 55555;
    /// UDP port for audio streaming (PC -> Mobile).
    pub const AUDIO_UDP: u16 = 55556;
    /// TCP port for ADB-tunneled audio framing.
    pub const ADB_AUDIO_TCP: u16 = 55557;
    /// TCP port for ADB-tunneled discovery/keepalive.
    pub const ADB_DISCOVERY_TCP: u16 = 55558;
}

// Keep legacy constants for backward compat during migration
pub const DISCOVERY_PORT: u16 = Ports::DISCOVERY;
pub const AUDIO_PORT: u16 = Ports::AUDIO_UDP;

pub const CPAL_BUFFER_SIZE: u32 = 512;

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
    // Use the TTL-cached interface list instead of the synchronous
    // `netdev::get_default_interface()` which triggers a 5-50ms OS syscall
    // on every call. This is critical because `get_local_ip()` is called
    // from the Presence broadcast loop and other hot paths.
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

pub fn classify_interface(interface: &netdev::Interface) -> (bool, bool) {
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

    cached_interfaces().iter().any(|iface| {
        let (_, is_usb) = classify_interface(iface);
        is_usb && iface.ipv4.iter().any(|net| {
            let local = net.addr().octets();
            let target = ipv4.octets();
            local[0] == target[0] && local[1] == target[1] && local[2] == target[2]
        })
    })
}

