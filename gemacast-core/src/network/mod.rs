pub mod discovery;
pub mod receiver;
pub mod sender;

pub use discovery::{
    DiscoveryBroadcaster, DiscoveryBroadcasterHandles, DiscoveryListener, DiscoveryListenerHandles,
};
pub use receiver::{AudioReceiver, AudioReceiverHandles};
pub use sender::{AudioSender, SenderCommand};

pub const DISCOVERY_PORT: u16 = 55555;
pub const AUDIO_PORT: u16 = 55556;

pub fn get_local_ip() -> Result<std::net::IpAddr, local_ip_address::Error> {
    local_ip_address::local_ip()
}

/// Computes the subnet-directed broadcast address using a routing hack.
/// On Android API 30+, `local_ip_address` often fails or returns localhost
/// due to strict permission models hiding network interfaces (`getifaddrs` restricted).
pub fn get_broadcast_addr() -> std::net::Ipv4Addr {
    let local_ip = match std::net::UdpSocket::bind("0.0.0.0:0") {
        Ok(socket) => {
            // Connect to a public IP (Google DNS) to force the OS to resolve the 
            // outbound route and assign the correct local interface IP.
            if socket.connect("8.8.8.8:80").is_ok() {
                if let Ok(std::net::SocketAddr::V4(addr)) = socket.local_addr() {
                    Some(*addr.ip())
                } else {
                    None
                }
            } else {
                None
            }
        }
        _ => None,
    };

    let Some(ip) = local_ip else {
        return std::net::Ipv4Addr::BROADCAST;
    };

    // We assume a standard /24 subnet (which covers 99% of home Wi-Fi networks).
    let octets = ip.octets();
    std::net::Ipv4Addr::new(octets[0], octets[1], octets[2], 255)
}
