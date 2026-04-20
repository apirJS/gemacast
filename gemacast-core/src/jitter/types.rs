use std::time::Instant;

/// A raw Opus packet received from the network, awaiting decode in the audio callback.
///
/// Stored undecoded so the Opus decoder lives entirely on the audio thread,
/// which is required for Opus PLC (packet loss concealment) to function correctly —
/// PLC depends on the decoder's internal state from the previous good frame.
pub struct RawPacket {
    /// Sender's monotonic sequence number (u64, big-endian over the wire).
    pub seq_num: u64,
    /// Payload bytes (Opus encoded or raw PCM). Vec<u8> ensures 0-copy moves across thread channels.
    pub payload_data: Vec<u8>,
    /// Actual length of valid data in the payload buffer
    pub payload_len: usize,
    /// Wall-clock time this packet arrived on the network thread.
    /// Used by the jitter controller to build the inter-arrival delay histogram.
    pub arrival_time: Instant,
    /// True if the payload_data is raw f32 PCM float bytes, False if Opus.
    pub is_uncompressed: bool,
    /// True if this represents a 100% silent frame (Opus encoder bypassed). `payload_data` will be empty.
    pub is_silence: bool,
}
