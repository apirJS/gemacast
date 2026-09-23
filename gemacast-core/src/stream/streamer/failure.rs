use crate::domain::types::{AudioSource, DeviceId};
use std::net::SocketAddr;

/// Identifies the failed task so the engine can evict only its current generation.
#[derive(Debug)]
pub(crate) enum StreamTaskFailure {
    Capture {
        source: AudioSource,
        generation: u64,
        message: String,
    },
    UdpEncoder {
        source: AudioSource,
        generation: u64,
        encoder_generation: u64,
        target: SocketAddr,
        message: String,
    },
    TcpEncoder {
        source: AudioSource,
        generation: u64,
        encoder_generation: u64,
        device_id: DeviceId,
        message: String,
    },
}
