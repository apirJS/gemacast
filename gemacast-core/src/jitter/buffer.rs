use super::types::RawPacket;

/// Number of packet slots in the jitter buffer.
/// 512 slots × 10ms/frame = 5.12s of maximum buffering headroom.
const BUFFER_CAPACITY: u64 = 512;

#[derive(PartialEq, Debug)]
pub enum InsertResult {
    Accepted,
    Stale,
    StreamRestarted,
}

/// A fixed-capacity circular buffer that reorders UDP packets by sequence number.
///
/// Design: Each slot corresponds to `seq_num % capacity`. Out-of-order packets
/// automatically land in the correct position. The buffer is entirely single-threaded —
/// it lives inside the audio callback.
pub struct JitterBuffer {
    slots: Vec<Option<RawPacket>>,
    capacity: u64,
    /// The sequence number we expect to play next.
    next_play_seq: u64,
    /// Whether we've received the first packet (to initialize `next_play_seq`).
    initialized: bool,
    /// O(1) count of filled slots. Maintained by insert / pop_next / reset.
    occupied: u32,
    /// Cached minimum sequence number for O(1) gap-path queries.
    /// `None` means the cache is stale and must be recomputed on next access.
    cached_min_seq: Option<u64>,
}

impl Default for JitterBuffer {
    fn default() -> Self {
        Self::new()
    }
}

impl JitterBuffer {
    pub fn new() -> Self {
        // We use an explicit manual layout because we can't use vec![None; CAPACITY]
        // without making the inner type Clone, and we don't want RawPacket to be Clone
        // to prevent accidental array duplications.
        let mut slots = Vec::with_capacity(BUFFER_CAPACITY as usize);
        for _ in 0..BUFFER_CAPACITY {
            slots.push(None);
        }
        Self {
            slots,
            capacity: BUFFER_CAPACITY,
            next_play_seq: 0,
            initialized: false,
            occupied: 0,
            cached_min_seq: None,
        }
    }

    /// Insert a packet into its sequence-ordered slot.
    pub fn insert(&mut self, packet: RawPacket) -> InsertResult {
        if !self.initialized {
            self.next_play_seq = packet.seq_num;
            self.initialized = true;
        }

        let mut restarted = false;
        if packet.seq_num < self.next_play_seq {
            if self.next_play_seq.saturating_sub(packet.seq_num) > self.capacity {
                self.reset();
                self.next_play_seq = packet.seq_num;
                self.initialized = true;
                restarted = true;
            } else {
                return InsertResult::Stale;
            }
        }

        if packet.seq_num >= self.next_play_seq + self.capacity {
            self.skip_to(packet.seq_num.saturating_sub(self.capacity / 2));
        }

        let seq = packet.seq_num;
        let index = (seq % self.capacity) as usize;
        if self.slots[index].is_none() {
            self.occupied += 1;
        }
        self.slots[index] = Some(packet);
        self.cached_min_seq = Some(self.cached_min_seq.map_or(seq, |m| m.min(seq)));

        if restarted {
            InsertResult::StreamRestarted
        } else {
            InsertResult::Accepted
        }
    }

    /// Try to pop the next expected packet.
    ///
    /// Returns `Some(packet)` if the slot has the exact expected packet.
    /// Returns `None` for a true gap (advances `next_play_seq` past the missing slot).
    /// Returns `None` without advancing if the slot holds a future packet.
    pub fn pop_next(&mut self) -> Option<RawPacket> {
        if !self.initialized {
            return None;
        }

        let index = (self.next_play_seq % self.capacity) as usize;
        let packet = self.slots[index].take();

        match packet {
            Some(pkt) if pkt.seq_num == self.next_play_seq => {
                self.occupied = self.occupied.saturating_sub(1);
                self.next_play_seq += 1;
                self.cached_min_seq = None;
                Some(pkt)
            }
            Some(pkt) => {
                if pkt.seq_num > self.next_play_seq {
                    self.slots[index] = Some(pkt);
                    self.next_play_seq += 1;
                } else {
                    self.occupied = self.occupied.saturating_sub(1);
                    self.next_play_seq += 1;
                }
                self.cached_min_seq = None;
                None
            }
            None => {
                self.next_play_seq += 1;
                None
            }
        }
    }

    pub fn advance_one(&mut self) {
        let index = (self.next_play_seq % self.capacity) as usize;
        if self.slots[index]
            .as_ref()
            .is_some_and(|p| p.seq_num == self.next_play_seq)
        {
            self.slots[index] = None;
            self.occupied = self.occupied.saturating_sub(1);
        }
        self.next_play_seq += 1;
        self.cached_min_seq = None;
    }

    /// O(1) count of filled slots.
    pub fn occupied_count(&self) -> u32 {
        if !self.initialized {
            return 0;
        }
        self.occupied
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

    /// Represents the total buffer occupancy depth to prevent prebuffer deadlocking on dropped packets.
    /// **Warning**: O(n) scan of all 512 slots. Only suitable for tests, not the hot path.
    pub fn contiguous_depth(&self) -> u32 {
        if !self.initialized {
            return 0;
        }
        let mut count = 0u32;
        for i in 0..self.capacity {
            let index = ((self.next_play_seq + i) % self.capacity) as usize;
            if let Some(pkt) = &self.slots[index]
                && pkt.seq_num >= self.next_play_seq
            {
                count += 1; // Pure occupancy count
            }
        }
        count
    }

    /// Skip the playhead forward to `new_seq`, clearing any slots in between.
    fn skip_to(&mut self, new_seq: u64) {
        let skip_distance = new_seq.saturating_sub(self.next_play_seq);
        if skip_distance >= self.capacity {
            // Full clear is faster than per-slot iteration when skipping >= capacity.
            for slot in &mut self.slots {
                *slot = None;
            }
            self.occupied = 0;
        } else {
            let clear_end = new_seq.min(self.next_play_seq + self.capacity);
            for seq in self.next_play_seq..clear_end {
                let index = (seq % self.capacity) as usize;
                if self.slots[index].is_some() {
                    self.slots[index] = None;
                    self.occupied = self.occupied.saturating_sub(1);
                }
            }
        }
        self.next_play_seq = new_seq;
        self.cached_min_seq = None;
    }

    /// Reset all state. Called on disconnect/reconnect.
    pub fn reset(&mut self) {
        for slot in &mut self.slots {
            *slot = None;
        }
        self.next_play_seq = 0;
        self.initialized = false;
        self.occupied = 0;
        self.cached_min_seq = None;
    }

    /// Read the sequence number the buffer expects to play next.
    pub fn next_play_seq(&self) -> u64 {
        self.next_play_seq
    }

    /// Find the lowest sequence number currently in the buffer slots.
    /// Uses a lazy cache: O(1) when cache is valid, O(n) recompute on first access after mutation.
    pub fn lowest_available_seq(&mut self) -> Option<u64> {
        if let Some(min) = self.cached_min_seq {
            return Some(min);
        }
        let mut min_seq = None;
        for pkt in self.slots.iter().flatten() {
            min_seq = match min_seq {
                None => Some(pkt.seq_num),
                Some(m) => Some(std::cmp::min(m, pkt.seq_num)),
            };
        }
        self.cached_min_seq = min_seq;
        min_seq
    }

    /// Fast-forward the playhead to `next_seq`, clearing any skipped slots.
    ///
    /// Delegates to `skip_to`, which correctly clears stale slot data and
    /// decrements the `occupied` counter for every skipped position.
    /// Calling this without clearing would leave stale packets in the circular
    /// buffer, causing `occupied_count()` to overcount and `lowest_available_seq()`
    /// to return stale (already-skipped) sequence numbers.
    pub fn fast_forward(&mut self, next_seq: u64) {
        self.skip_to(next_seq);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_packet(seq: u64) -> RawPacket {
        let mut pkt = RawPacket::zeroed();
        pkt.seq_num = seq;
        pkt.payload_len = 100;
        pkt
    }

    #[test]
    fn insert_and_pop_in_order() {
        let mut buf = JitterBuffer::new();
        matches!(buf.insert(make_packet(0)), InsertResult::Accepted);
        matches!(buf.insert(make_packet(1)), InsertResult::Accepted);
        matches!(buf.insert(make_packet(2)), InsertResult::Accepted);

        assert_eq!(buf.contiguous_depth(), 3);
        assert_eq!(buf.occupied_count(), 3);
        assert!(buf.pop_next().is_some_and(|p| p.seq_num == 0));
        assert!(buf.pop_next().is_some_and(|p| p.seq_num == 1));
        assert!(buf.pop_next().is_some_and(|p| p.seq_num == 2));
        assert_eq!(buf.occupied_count(), 0);
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
        buf.insert(make_packet(2)); // skip seq 1

        assert!(buf.pop_next().is_some_and(|p| p.seq_num == 0));
        assert!(buf.pop_next().is_none()); // seq 1 missing
        assert!(buf.pop_next().is_some_and(|p| p.seq_num == 2));
    }

    #[test]
    fn rejects_stale_packets() {
        let mut buf = JitterBuffer::new();
        buf.insert(make_packet(5));
        assert_eq!(InsertResult::Stale, buf.insert(make_packet(3)))
    }

    #[test]
    fn advance_one_skips_without_corrupting_future_slots() {
        let mut buf = JitterBuffer::new();
        buf.insert(make_packet(0));
        buf.insert(make_packet(2)); // gap at 1

        assert!(buf.pop_next().is_some_and(|p| p.seq_num == 0)); // pop 0
        // seq 1 is missing; advance_one() declares it lost without touching slot 2

        buf.advance_one();
        assert!(buf.pop_next().is_some_and(|p| p.seq_num == 2)); // 2 still intact
    }
}
