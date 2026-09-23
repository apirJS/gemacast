use std::time::{Duration, Instant};

/// Converts sustained silence into suspend and wake edges.
pub(crate) struct SourceIdleDetector {
    idle_after: Duration,
    silence_started_at: Option<Instant>,
    suspended: bool,
}

impl SourceIdleDetector {
    pub(crate) fn new(idle_after: Duration) -> Self {
        Self {
            idle_after,
            silence_started_at: None,
            suspended: false,
        }
    }

    pub(crate) fn observe(&mut self, is_silence: bool, now: Instant) -> Option<bool> {
        if !is_silence {
            self.silence_started_at = None;
            if self.suspended {
                self.suspended = false;
                return Some(false);
            }
            return None;
        }

        let started = *self.silence_started_at.get_or_insert(now);
        if !self.suspended && now.saturating_duration_since(started) >= self.idle_after {
            self.suspended = true;
            return Some(true);
        }

        None
    }

    pub(crate) fn is_suspended(&self) -> bool {
        self.suspended
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silence_must_cross_the_full_grace_period_before_suspending() {
        let base = Instant::now();
        let mut detector = SourceIdleDetector::new(Duration::from_secs(5));

        assert_eq!(detector.observe(true, base), None);
        assert_eq!(
            detector.observe(true, base + Duration::from_millis(4_999)),
            None
        );
        assert!(!detector.is_suspended());
        assert_eq!(
            detector.observe(true, base + Duration::from_secs(5)),
            Some(true)
        );
        assert!(detector.is_suspended());
    }

    #[test]
    fn a_suspended_source_emits_only_one_idle_edge() {
        let base = Instant::now();
        let mut detector = SourceIdleDetector::new(Duration::from_secs(1));

        assert_eq!(detector.observe(true, base), None);
        assert_eq!(
            detector.observe(true, base + Duration::from_secs(1)),
            Some(true)
        );
        assert_eq!(detector.observe(true, base + Duration::from_secs(30)), None);
    }

    #[test]
    fn the_first_real_packet_wakes_a_suspended_source_once() {
        let base = Instant::now();
        let mut detector = SourceIdleDetector::new(Duration::from_secs(1));

        detector.observe(true, base);
        detector.observe(true, base + Duration::from_secs(1));

        assert_eq!(
            detector.observe(false, base + Duration::from_secs(2)),
            Some(false)
        );
        assert!(!detector.is_suspended());
        assert_eq!(detector.observe(false, base + Duration::from_secs(3)), None);
    }

    #[test]
    fn a_short_silence_after_waking_starts_a_fresh_grace_period() {
        let base = Instant::now();
        let mut detector = SourceIdleDetector::new(Duration::from_secs(5));

        detector.observe(true, base);
        detector.observe(false, base + Duration::from_secs(2));

        assert_eq!(detector.observe(true, base + Duration::from_secs(3)), None);
        assert_eq!(detector.observe(true, base + Duration::from_secs(7)), None);
        assert_eq!(
            detector.observe(true, base + Duration::from_secs(8)),
            Some(true)
        );
    }
}
