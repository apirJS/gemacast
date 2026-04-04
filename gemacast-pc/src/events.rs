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
    #[allow(dead_code)] // Used internally by the background engine dispatch path
    AddTarget(SocketAddr),
    RemoveTarget(SocketAddr, String),
    StopStream,
    StartBroadcasting,
    StopBroadcasting,
}
