use std::time::Instant;

/// A raw Opus packet received from the network, awaiting decode in the audio callback.
///
/// Stored undecoded so the Opus decoder lives entirely on the audio thread,
/// which is required for Opus PLC (packet loss concealment) to function correctly —
/// PLC depends on the decoder's internal state from the previous good frame.
pub struct RawPacket {
    /// Sender's monotonic sequence number (u64, big-endian over the wire).
    pub seq_num: u64,
    /// The raw Opus-encoded bytes (typically 50–300 bytes per 10ms frame).
    pub opus_data: Vec<u8>,
    /// Wall-clock time this packet arrived on the network thread.
    /// Used by the jitter controller to build the inter-arrival delay histogram.
    pub arrival_time: Instant,
}


