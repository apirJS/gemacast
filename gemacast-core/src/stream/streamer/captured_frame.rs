use std::time::Duration;
use tokio::time::Instant;

#[derive(Debug)]
pub(crate) struct CapturedFrame {
    pub samples: Vec<f32>,
    pub position: u64,
    pub ready_at: Instant,
}

impl CapturedFrame {
    pub const DURATION: Duration = Duration::from_millis(10);
    // Transport budget: never enqueue more than four frames of old live audio.
    pub const MAX_SEND_AGE: Duration = Duration::from_millis(40);

    pub fn expired(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.ready_at) >= Self::MAX_SEND_AGE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captured_audio_should_expire_at_its_delivery_deadline() {
        let start = Instant::now();
        let frame = CapturedFrame {
            samples: vec![],
            position: 5,
            ready_at: start,
        };
        assert!(!frame.expired(start + Duration::from_millis(39)));
        assert!(frame.expired(start + Duration::from_millis(40)));
    }
}
