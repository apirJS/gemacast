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

#[derive(Debug, Clone)]
pub enum DaemonCommand {
    KickDevice(DeviceId),
    StopAllStreams,
    StartBroadcasting,
    StopBroadcasting,
    ChangeBitrate(Option<i32>),
}
