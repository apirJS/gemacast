use super::types::RawPacket;

/// Number of packet slots in the jitter buffer.
/// 512 slots × 10ms/frame = 5.12s of maximum buffering headroom.
const BUFFER_CAPACITY: u64 = 512;

/// A fixed-capacity circular buffer that reorders UDP packets by sequence number.
///
/// Design: Each slot corresponds to `seq_num % capacity`. Out-of-order packets
/// automatically land in the correct position. The buffer is entirely single-threaded —
/// it lives inside the cpal audio callback closure.
pub struct JitterBuffer {
    slots: Vec<Option<RawPacket>>,
    capacity: u64,
    /// The sequence number we expect to play next.
    next_play_seq: u64,
    /// Whether we've received the first packet (to initialize `next_play_seq`).
    initialized: bool,
}

impl Default for JitterBuffer {
    fn default() -> Self {
        Self::new()
    }
}

impl JitterBuffer {
    pub fn new() -> Self {
        let slots = (0..BUFFER_CAPACITY).map(|_| None).collect();
        Self {
            slots,
            capacity: BUFFER_CAPACITY,
            next_play_seq: 0,
            initialized: false,
        }
    }

    /// Insert a packet into its sequence-ordered slot.
    ///
    /// Returns `true` if the packet was accepted, `false` if it was stale
    /// (sequence number already played).
    pub fn insert(&mut self, packet: RawPacket) -> bool {
        if !self.initialized {
            // First packet ever — anchor our playback sequence to it.
            self.next_play_seq = packet.seq_num;
            self.initialized = true;
        }

        // Reject packets we've already played past.
        if packet.seq_num < self.next_play_seq {
            // If it's outrageously in the past (e.g. sequence went from 500 to 0) 
            // It strongly implies the sender process crashed and restarted completely natively.
            if self.next_play_seq.saturating_sub(packet.seq_num) > self.capacity {
                self.reset();
                self.next_play_seq = packet.seq_num;
                self.initialized = true;
            } else {
                return false;
            }
        }

        // If the packet is impossibly far ahead, we likely lost a burst of packets.
        // Skip forward to keep the pipeline flowing rather than stalling forever.
        if packet.seq_num >= self.next_play_seq + self.capacity {
            self.skip_to(packet.seq_num.saturating_sub(self.capacity / 2));
        }

        let index = (packet.seq_num % self.capacity) as usize;
        self.slots[index] = Some(packet);
        true
    }

    /// Try to pop the next expected packet.
    ///
    /// Always advances `next_play_seq` — even when the packet is missing.
    /// This prevents the buffer from getting stuck waiting for a packet
    /// that will never arrive (UDP has no retransmission).
    pub fn pop_next(&mut self) -> Option<RawPacket> {
        if !self.initialized {
            return None;
        }

        let index = (self.next_play_seq % self.capacity) as usize;
        let packet = self.slots[index].take();

        match packet {
            Some(pkt) if pkt.seq_num == self.next_play_seq => {
                // Correct packet — advance playhead.
                self.next_play_seq += 1;
                Some(pkt)
            }
            Some(pkt) => {
                // Wrong packet in this slot (stale from a previous wraparound).
                // Put it back if it's a future packet, otherwise discard.
                if pkt.seq_num > self.next_play_seq {
                    self.slots[index] = Some(pkt);
                }
                self.next_play_seq += 1;
                None
            }
            None => {
                // Missing packet — advance anyway.
                self.next_play_seq += 1;
                None
            }
        }
    }

    /// Count how many physical packets are in the buffer regardless of sequence gaps.
    pub fn occupied_count(&self) -> u32 {
        if !self.initialized {
            return 0;
        }
        self.slots.iter().filter(|s| s.is_some()).count() as u32
    }

    /// Check if the next expected packet is present without consuming it.
    pub fn has_next(&self) -> bool {
        if !self.initialized {
            return false;
        }
        let index = (self.next_play_seq % self.capacity) as usize;
        self.slots[index]
            .as_ref()
            .is_some_and(|pkt| pkt.seq_num == self.next_play_seq)
    }

    /// Count how many sequential packets are available starting from `next_play_seq`.
    ///
    /// This is the "buffer depth" — how many frames we could play before running dry.
    pub fn contiguous_depth(&self) -> u32 {
        if !self.initialized {
            return 0;
        }
        let mut count = 0u32;
        let mut seq = self.next_play_seq;
        loop {
            let index = (seq % self.capacity) as usize;
            match &self.slots[index] {
                Some(pkt) if pkt.seq_num == seq => {
                    count += 1;
                    seq += 1;
                    if count >= self.capacity as u32 {
                        break;
                    }
                }
                _ => break,
            }
        }
        count
    }

    /// Skip the playhead forward to `new_seq`, clearing any slots in between.
    fn skip_to(&mut self, new_seq: u64) {
        let clear_end = new_seq.min(self.next_play_seq + self.capacity);
        for seq in self.next_play_seq..clear_end {
            let index = (seq % self.capacity) as usize;
            self.slots[index] = None;
        }
        self.next_play_seq = new_seq;
    }

    /// Reset all state. Called on disconnect/reconnect.
    pub fn reset(&mut self) {
        for slot in &mut self.slots {
            *slot = None;
        }
        self.next_play_seq = 0;
        self.initialized = false;
    }

    /// Read the sequence number the buffer expects to play next.
    pub fn next_play_seq(&self) -> u64 {
        self.next_play_seq
    }

    /// Find the lowest sequence number currently residing in the buffer slots.
    pub fn lowest_available_seq(&self) -> Option<u64> {
        let mut min_seq = None;
        for slot in &self.slots {
            if let Some(pkt) = slot {
                min_seq = match min_seq {
                    None => Some(pkt.seq_num),
                    Some(m) => Some(std::cmp::min(m, pkt.seq_num)),
                };
            }
        }
        min_seq
    }

    /// Fast-forwards the expected playback sequence, instantly dropping any theoretical 
    /// packets between the old sequence and the new sequence.
    pub fn fast_forward(&mut self, next_seq: u64) {
        self.next_play_seq = next_seq;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn make_packet(seq: u64) -> RawPacket {
        RawPacket {
            seq_num: seq,
            payload_data: vec![0u8; 100],
            arrival_time: Instant::now(),
            is_uncompressed: false,
        }
    }

    #[test]
    fn insert_and_pop_in_order() {
        let mut buf = JitterBuffer::new();
        buf.insert(make_packet(0));
        buf.insert(make_packet(1));
        buf.insert(make_packet(2));

        assert_eq!(buf.contiguous_depth(), 3);
        assert!(buf.pop_next().is_some_and(|p| p.seq_num == 0));
        assert!(buf.pop_next().is_some_and(|p| p.seq_num == 1));
        assert!(buf.pop_next().is_some_and(|p| p.seq_num == 2));
        assert!(buf.pop_next().is_none());
    }

    #[test]
    fn reorders_out_of_order_packets() {
        let mut buf = JitterBuffer::new();
        buf.insert(make_packet(2));
        buf.insert(make_packet(0));
        buf.insert(make_packet(1));

        // First packet was seq=2, so next_play_seq anchored to 2.
        // Seq 0 and 1 are stale → rejected.
        assert_eq!(buf.contiguous_depth(), 1);
        assert!(buf.pop_next().is_some_and(|p| p.seq_num == 2));
    }

    #[test]
    fn reorders_when_first_packet_is_earliest() {
        let mut buf = JitterBuffer::new();
        buf.insert(make_packet(0));
        buf.insert(make_packet(2));
        buf.insert(make_packet(1));

        assert_eq!(buf.contiguous_depth(), 3);
        assert!(buf.pop_next().is_some_and(|p| p.seq_num == 0));
        assert!(buf.pop_next().is_some_and(|p| p.seq_num == 1));
        assert!(buf.pop_next().is_some_and(|p| p.seq_num == 2));
    }

    #[test]
    fn missing_packet_returns_none_and_advances() {
        let mut buf = JitterBuffer::new();
        buf.insert(make_packet(0));
        // Skip seq 1
        buf.insert(make_packet(2));

        assert!(buf.pop_next().is_some_and(|p| p.seq_num == 0));
        assert!(buf.pop_next().is_none()); // seq 1 missing
        assert!(buf.pop_next().is_some_and(|p| p.seq_num == 2));
    }

    #[test]
    fn rejects_stale_packets() {
        let mut buf = JitterBuffer::new();
        buf.insert(make_packet(5));
        assert!(!buf.insert(make_packet(3))); // stale
    }
}
