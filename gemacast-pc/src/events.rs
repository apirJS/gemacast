use std::net::SocketAddr;

pub enum DaemonEvent {
    DiscoveredDevice {
        device_id: String,
        name: String,
        addr: SocketAddr,
    },
    DeviceLost(String, SocketAddr),
    FatalError(String),
}

#[derive(Debug)]
pub enum StreamCommand {
    AddTarget(SocketAddr),
    RemoveTarget(SocketAddr),
    StopStream,
}
