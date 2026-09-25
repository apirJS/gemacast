use crate::audio::pcm_datagram::{PcmChunk, PcmDatagram};
use crate::jitter::RawPacket;
use std::time::{Duration, Instant};

const SLOTS: usize = 16;
// Incomplete frames have a finite lifetime even when playback is paused.
const ASSEMBLY_DEADLINE: Duration = Duration::from_millis(100);

#[derive(Default)]
struct FrameSlot {
    sequence: Option<u64>,
    first_arrival: Option<Instant>,
    mask: u8,
    packet: Option<RawPacket>,
}

/// Reassembles transport chunks without changing the jitter buffer's 10 ms units.
pub(crate) struct PcmAssembly {
    generation: Option<u64>,
    newest_sequence: u64,
    slots: Vec<FrameSlot>,
    pub completed_frames: u64,
    pub expired_frames: u64,
}

impl PcmAssembly {
    pub fn new() -> Self {
        Self {
            generation: None,
            newest_sequence: 0,
            slots: (0..SLOTS).map(|_| FrameSlot::default()).collect(),
            completed_frames: 0,
            expired_frames: 0,
        }
    }

    pub fn push(&mut self, chunk: PcmChunk<'_>, now: Instant) -> Option<RawPacket> {
        self.expire(now);
        if self
            .generation
            .is_some_and(|generation| chunk.generation < generation)
        {
            return None;
        }
        if self.generation != Some(chunk.generation) {
            self.slots
                .iter_mut()
                .for_each(|slot| *slot = FrameSlot::default());
            self.generation = Some(chunk.generation);
            self.newest_sequence = chunk.sequence;
        }
        self.newest_sequence = self.newest_sequence.max(chunk.sequence);
        if self.newest_sequence.saturating_sub(chunk.sequence) >= SLOTS as u64 {
            return None;
        }
        let slot = &mut self.slots[chunk.sequence as usize % SLOTS];
        if slot.sequence != Some(chunk.sequence) {
            if slot.packet.is_some() {
                self.expired_frames += 1;
            }
            let mut packet = RawPacket::zeroed();
            packet.seq_num = chunk.sequence;
            packet.is_uncompressed = true;
            packet.payload_len = PcmDatagram::PAYLOAD * PcmDatagram::CHUNKS;
            *slot = FrameSlot {
                sequence: Some(chunk.sequence),
                first_arrival: Some(now),
                mask: 0,
                packet: Some(packet),
            };
        }
        let packet = slot.packet.as_mut()?;
        if slot.mask & (1 << chunk.index) != 0 {
            return None;
        }
        let offset = chunk.index * PcmDatagram::PAYLOAD;
        packet.payload_data[offset..offset + PcmDatagram::PAYLOAD].copy_from_slice(chunk.payload);
        slot.mask |= 1 << chunk.index;
        if slot.mask != 0b1111 {
            return None;
        }
        // A frame becomes available to playback only when all its bytes have arrived.
        packet.arrival_time = now;
        self.completed_frames += 1;
        slot.packet.take()
    }

    pub fn expire(&mut self, now: Instant) {
        for slot in &mut self.slots {
            if slot.packet.is_some()
                && slot
                    .first_arrival
                    .is_some_and(|start| now.saturating_duration_since(start) >= ASSEMBLY_DEADLINE)
            {
                slot.packet = None;
                self.expired_frames += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn push(
        assembly: &mut PcmAssembly,
        sequence: u64,
        generation: u64,
        index: usize,
        now: Instant,
    ) -> Option<RawPacket> {
        assembly.push(
            PcmChunk {
                sequence,
                generation,
                index,
                payload: &[index as u8; PcmDatagram::PAYLOAD],
            },
            now,
        )
    }

    #[test]
    fn reordered_chunks_should_produce_exactly_one_complete_frame() {
        let mut assembly = PcmAssembly::new();
        let now = Instant::now();
        for index in [3, 0, 3, 2] {
            assert!(push(&mut assembly, 12, 1, index, now).is_none());
        }
        let packet = push(&mut assembly, 12, 1, 1, now).unwrap();
        assert_eq!(packet.seq_num, 12);
        assert_eq!(packet.payload_len, 3840);
        for index in 0..4 {
            let offset = index * PcmDatagram::PAYLOAD;
            assert_eq!(
                &packet.payload_data[offset..offset + PcmDatagram::PAYLOAD],
                &[index as u8; PcmDatagram::PAYLOAD]
            );
        }
        assert!(packet.is_uncompressed);
        for index in 0..4 {
            assert!(push(&mut assembly, 12, 1, index, now).is_none());
        }
    }

    #[test]
    fn an_expired_frame_should_not_be_resurrected_by_late_chunks() {
        let mut assembly = PcmAssembly::new();
        let now = Instant::now();
        push(&mut assembly, 12, 1, 0, now);
        for index in 1..4 {
            assert!(push(&mut assembly, 12, 1, index, now + ASSEMBLY_DEADLINE).is_none());
        }
        assert_eq!(assembly.expired_frames, 1);
        for index in 0..3 {
            push(&mut assembly, 13, 1, index, now + ASSEMBLY_DEADLINE);
        }
        assert!(push(&mut assembly, 13, 1, 3, now + ASSEMBLY_DEADLINE).is_some());
    }

    #[test]
    fn old_generations_should_never_replace_a_new_stream() {
        let mut assembly = PcmAssembly::new();
        let now = Instant::now();
        push(&mut assembly, 12, 1, 0, now);
        push(&mut assembly, 12, 2, 1, now);
        for index in 0..4 {
            assert!(push(&mut assembly, 12, 1, index, now).is_none());
        }
        for index in [0, 2] {
            push(&mut assembly, 12, 2, index, now);
        }
        assert!(push(&mut assembly, 12, 2, 3, now).is_some());
    }

    #[test]
    fn chunks_outside_the_reordering_window_should_not_evict_recent_audio() {
        let mut assembly = PcmAssembly::new();
        let now = Instant::now();
        push(&mut assembly, 32, 1, 0, now);
        for index in 0..4 {
            assert!(push(&mut assembly, 16, 1, index, now).is_none());
        }
        for index in 1..3 {
            push(&mut assembly, 32, 1, index, now);
        }
        assert!(push(&mut assembly, 32, 1, 3, now).is_some());
    }

    #[test]
    fn an_incomplete_frame_should_expire_without_another_pcm_chunk() {
        let mut assembly = PcmAssembly::new();
        let start = Instant::now();
        assert!(push(&mut assembly, 12, 1, 0, start).is_none());
        assembly.expire(start + ASSEMBLY_DEADLINE);
        assert_eq!(assembly.expired_frames, 1);
        for index in 1..4 {
            assert!(push(&mut assembly, 12, 1, index, start + ASSEMBLY_DEADLINE).is_none());
        }
        assert_eq!(assembly.expired_frames, 1);
    }
}
