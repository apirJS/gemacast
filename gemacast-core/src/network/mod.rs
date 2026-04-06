pub mod discovery;
pub mod receiver;
pub mod sender;

pub use discovery::{
    DiscoveryBroadcaster, DiscoveryBroadcasterHandles, DiscoveryListener, DiscoveryListenerHandles, send_control_message
};
pub use receiver::{AudioReceiver, AudioReceiverHandles};
pub use sender::{AudioSender, SenderCommand};

pub const DISCOVERY_PORT: u16 = 55555;
pub const AUDIO_PORT: u16 = 55556;

/// The specific buffer size (in samples) handed to CPAL. 
/// 480 samples = 10ms at 48kHz. Set to a lower number for faster audio loop triggers.
pub const CPAL_BUFFER_SIZE: u32 = 512;

pub fn get_local_ip() -> Result<std::net::IpAddr, local_ip_address::Error> {
    local_ip_address::local_ip()
}

/// Computes the subnet-directed broadcast address using a routing hack.
/// On Android API 30+, `local_ip_address` often fails or returns localhost
/// due to strict permission models hiding network interfaces (`getifaddrs` restricted).
pub fn get_broadcast_addrs() -> Vec<std::net::Ipv4Addr> {
    let mut addrs = Vec::new();

    if let Ok(interfaces) = local_ip_address::list_afinet_netifas() {
        for (_, ip) in interfaces {
            if let std::net::IpAddr::V4(ipv4) = ip
                && !ipv4.is_loopback()
            {
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
