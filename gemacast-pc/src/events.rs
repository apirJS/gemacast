use gemacast_core::types::DeviceId;
use std::net::SocketAddr;

pub enum DaemonEvent {
    DiscoveredDevice {
        device_id: DeviceId,
        name: String,
        addr: SocketAddr,
    },
    DeviceLost(DeviceId, SocketAddr),
    FatalError(String),
}

#[derive(Debug)]
pub enum StreamCommand {
    #[allow(dead_code)]
    AddTarget(SocketAddr),
    RemoveTarget(SocketAddr, DeviceId),
    StopStream,
    StartBroadcasting,
    StopBroadcasting,
    ChangeBitrate(Option<i32>),
}
