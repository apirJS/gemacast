use std::time::Instant;

use crate::audio::MAX_OPUS_PACKET_SIZE;

/// Maximum payload size for a single audio packet.
/// Covers both Opus (typically <500 bytes) and raw uncompressed PCM (7680 bytes).
pub const MAX_PACKET_PAYLOAD: usize = MAX_OPUS_PACKET_SIZE;

/// A raw Opus packet received from the network, awaiting decode in the audio callback.
///
/// Stored undecoded so the Opus decoder lives entirely on the audio thread,
/// which is required for Opus PLC (packet loss concealment) to function correctly —
/// PLC depends on the decoder's internal state from the previous good frame.
///
/// Uses a fixed-size inline array instead of `Vec<u8>` to eliminate per-packet
/// heap allocations on the network receive thread (~200 allocs/sec at 5ms frame interval).
/// The SPSC ring buffer pre-allocates all slots at startup, so the total memory
/// cost is fixed and paid once.
pub struct RawPacket {
    /// Sender's monotonic sequence number (u64, big-endian over the wire).
    pub seq_num: u64,
    /// Payload bytes (Opus encoded or raw PCM). Fixed-size inline buffer.
    pub payload_data: [u8; MAX_PACKET_PAYLOAD],
    /// Actual length of valid data in the payload buffer.
    pub payload_len: usize,
    /// Wall-clock time this packet arrived on the network thread.
    /// Used by the jitter controller to build the inter-arrival delay histogram.
    pub arrival_time: Instant,
    /// True if the payload_data is raw f32 PCM float bytes, False if Opus.
    pub is_uncompressed: bool,
    /// True if this represents a 100% silent frame (Opus encoder bypassed). `payload_data` will be empty.
    pub is_silence: bool,
}

impl RawPacket {
    /// Create a zero-initialized packet. Used as a placeholder during
    /// buffer/test setup — real packets are populated by `parse_packet`.
    pub fn zeroed() -> Self {
        Self {
            seq_num: 0,
            payload_data: [0u8; MAX_PACKET_PAYLOAD],
            payload_len: 0,
            arrival_time: Instant::now(),
            is_uncompressed: false,
            is_silence: false,
        }
    }
}
