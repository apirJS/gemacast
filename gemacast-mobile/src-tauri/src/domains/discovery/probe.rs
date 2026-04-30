use gemacast_core::network::Ports;
use gemacast_core::types::{ConnectionMode, DeviceId};
use std::sync::Arc;
use tokio::net::UdpSocket;

pub async fn run_probe_loop(
    socket: Arc<UdpSocket>,
    device_id: DeviceId,
    mode: ConnectionMode,
) {
    if mode == ConnectionMode::Adb {
        return;
    }

    let mut interval = tokio::time::interval(tokio::time::Duration::from_millis(1000));
    let payload = gemacast_core::types::ControlMessage::Probe {
        device_id: Some(device_id),
    };

    let Ok(json_bytes) = serde_json::to_vec(&payload) else {
        return;
    };

    loop {
        interval.tick().await;

        for _ in 0..2 {
            let subnets = collect_local_subnets();
            for (b0, b1, b2) in subnets {
                for host in 1..=254u8 {
                    let target = std::net::SocketAddrV4::new(
                        std::net::Ipv4Addr::new(b0, b1, b2, host),
                        Ports::DISCOVERY,
                    );
                    let _ = socket.send_to(&json_bytes, target).await;
                }
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        }
    }
}

fn collect_local_subnets() -> Vec<(u8, u8, u8)> {
    let mut subnets = Vec::new();
    for iface in netdev::get_interfaces() {
        for ip_net in iface.ipv4 {
            let ip = ip_net.addr();
            if !ip.is_loopback() {
                let o = ip.octets();
                subnets.push((o[0], o[1], o[2]));
            }
        }
    }
    if subnets.is_empty() {
        subnets.push((192, 168, 42));
        subnets.push((192, 168, 43));
    }
    subnets
}
