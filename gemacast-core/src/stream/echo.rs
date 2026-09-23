//! UDP echo-ping helpers for measuring raw wire round-trip time (RTT).
//!
//! The phone piggybacks a tiny timestamped packet on its keepalive heartbeat
//! (every 500 ms) to the PC's audio socket; the PC bounces the bytes back
//! verbatim, and the phone subtracts the embedded send-time from now. This
//! measures the real UDP wire RTT - single-digit ms on a good LAN - unlike the
//! old control-channel probe, which timed a full cold TLS handshake and read
//! 50-75 ms on the same link.
//!
//! The packet is 10 bytes: a 2-byte magic prefix + an 8-byte big-endian
//! microsecond timestamp relative to `PING_EPOCH`. The magic cannot collide
//! with an audio packet on the shared socket: audio packets begin with the top
//! two bytes of a milliseconds-since-epoch sequence number, which stay
//! `0x00 0x00` for centuries, and `AudioPacketDecoder` rejects anything shorter than
//! 9 bytes anyway. The PC never interprets the payload; it only bounces the
//! exact bytes back, so all timestamp logic lives on the phone.

use std::sync::LazyLock;
use std::time::Instant;

/// Distinguishes an echo ping from an audio packet on the shared UDP socket.
const MAGIC: [u8; 2] = [0xF1, 0x1E];

/// Total wire size of an echo ping: 2-byte magic + 8-byte timestamp.
pub(crate) const ECHO_PACKET_LEN: usize = 10;

/// Process-wide monotonic base so a send-time stamped on the heartbeat thread
/// is comparable against a receive-time read on the receive thread. Both run in
/// the same process (the phone), so a single shared `Instant` origin makes the
/// two microsecond readings differenceable.
static PING_EPOCH: LazyLock<Instant> = LazyLock::new(Instant::now);

/// Owns the echo packet wire format and RTT calculation.
pub(crate) struct EchoPacket;

impl EchoPacket {
    pub(crate) fn build() -> [u8; ECHO_PACKET_LEN] {
        Self::encode_at(Self::now_micros())
    }

    pub(crate) fn matches(buf: &[u8], len: usize) -> bool {
        len == ECHO_PACKET_LEN && buf.len() >= ECHO_PACKET_LEN && buf[..2] == MAGIC
    }

    pub(crate) fn round_trip_ms(buf: &[u8]) -> f32 {
        Self::round_trip_ms_at(buf, Self::now_micros())
    }

    fn now_micros() -> u64 {
        PING_EPOCH.elapsed().as_micros() as u64
    }

    fn encode_at(send_micros: u64) -> [u8; ECHO_PACKET_LEN] {
        let mut buf = [0u8; ECHO_PACKET_LEN];
        buf[..2].copy_from_slice(&MAGIC);
        buf[2..].copy_from_slice(&send_micros.to_be_bytes());
        buf
    }

    fn round_trip_ms_at(buf: &[u8], now_micros: u64) -> f32 {
        let mut stamp = [0u8; 8];
        stamp.copy_from_slice(&buf[2..ECHO_PACKET_LEN]);
        let send_micros = u64::from_be_bytes(stamp);
        now_micros.saturating_sub(send_micros) as f32 / 1000.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::FORMAT_OPUS;

    #[test]
    fn build_ping_is_recognized_as_an_echo() {
        let ping = EchoPacket::build();
        assert!(EchoPacket::matches(&ping, ping.len()));
    }

    #[test]
    fn is_echo_rejects_the_old_single_byte_heartbeat() {
        assert!(!EchoPacket::matches(&[0u8], 1));
    }

    #[test]
    fn is_echo_rejects_a_ten_byte_audio_packet() {
        // 8-byte seq + 1-byte format flag + 1-byte payload == 10 bytes, the
        // one audio length that collides with ours. A realistic ms-epoch seq
        // has `0x00 0x00` leading bytes, so the magic never matches.
        let mut audio = [0u8; ECHO_PACKET_LEN];
        audio[0..8].copy_from_slice(&1_700_000_000_000u64.to_be_bytes());
        audio[8] = FORMAT_OPUS;
        audio[9] = 0x7F;
        assert!(!EchoPacket::matches(&audio, audio.len()));
    }

    #[test]
    fn is_echo_rejects_wrong_lengths() {
        let ping = EchoPacket::build();
        assert!(!EchoPacket::matches(&ping, ECHO_PACKET_LEN - 1));
        assert!(!EchoPacket::matches(&ping, ECHO_PACKET_LEN + 1));
    }

    #[test]
    fn read_rtt_ms_computes_elapsed_microseconds() {
        let ping = EchoPacket::encode_at(1_000);
        // 5_000 - 1_000 == 4_000 us == 4.0 ms.
        assert_eq!(EchoPacket::round_trip_ms_at(&ping, 5_000), 4.0);
    }

    #[test]
    fn read_rtt_ms_saturates_when_the_clock_appears_to_run_backwards() {
        let ping = EchoPacket::encode_at(9_000);
        assert_eq!(EchoPacket::round_trip_ms_at(&ping, 1_000), 0.0);
    }

    #[test]
    fn a_freshly_built_ping_reads_a_small_nonnegative_rtt() {
        let rtt = EchoPacket::round_trip_ms(&EchoPacket::build());
        assert!((0.0..1_000.0).contains(&rtt), "unexpected rtt {rtt}");
    }
}
