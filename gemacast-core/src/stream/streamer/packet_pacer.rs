use std::time::Duration;
use tokio::time::Instant;

pub(crate) struct PacketPacer {
    pub deadline: Instant,
}

impl PacketPacer {
    pub fn new(now: Instant) -> Self {
        Self { deadline: now }
    }

    pub fn sent(&mut self, now: Instant, period: Duration) {
        // Keep the media clock through timer rounding. After a stall, cap catch-up
        // at twice the nominal rate instead of dispatching all missed ticks at once.
        self.deadline = (self.deadline + period).max(now + period / 2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timer_rounding_should_not_reduce_the_audio_sample_rate() {
        let start = Instant::now();
        let mut pacer = PacketPacer::new(start);
        let period = Duration::from_millis(10);
        for tick in 0..100 {
            let actual = start + period * tick + Duration::from_micros(900);
            pacer.sent(actual, period);
        }
        assert_eq!(pacer.deadline, start + Duration::from_secs(1));
    }

    #[test]
    fn a_stalled_sender_should_not_replay_missed_deadlines_in_a_burst() {
        let start = Instant::now();
        let mut pacer = PacketPacer::new(start);
        let period = Duration::from_millis(10);
        let resumed = start + Duration::from_millis(100);
        pacer.sent(resumed, period);
        assert_eq!(pacer.deadline, resumed + period / 2);
        for _ in 0..10 {
            let actual = pacer.deadline;
            pacer.sent(actual, period);
            assert_eq!(pacer.deadline, actual + period);
        }
    }
}
