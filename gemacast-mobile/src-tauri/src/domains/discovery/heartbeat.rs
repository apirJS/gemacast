//! Extracted heartbeat timeout logic for testability.
//!
//! The watchdog tick body is pulled out of [`super::listener`] so it can
//! be unit-tested without timers or task spawning. Mirrors the pattern
//! from `gemacast-pc/src/tasks/device_watchdog.rs`.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use gemacast_core::types::DeviceId;

use crate::traits::FrontendNotifier;

/// Evict senders whose last heartbeat exceeds `timeout`.
///
/// Notifies the frontend for each evicted sender and removes them
/// from the tracker. Returns the IDs of evicted senders.
pub fn evict_stale_senders(
    notifier: &dyn FrontendNotifier,
    tracker: &Mutex<HashMap<DeviceId, Instant>>,
    timeout: Duration,
) -> Vec<DeviceId> {
    let stale: Vec<DeviceId> = {
        let map = tracker.lock().unwrap();
        let now = Instant::now();
        map.iter()
            .filter(|(_, ts)| now.duration_since(**ts) >= timeout)
            .map(|(id, _)| id.clone())
            .collect()
    };

    for sender_id in &stale {
        notifier.emit_sender_timeout(sender_id);
    }

    if !stale.is_empty() {
        let mut map = tracker.lock().unwrap();
        for id in &stale {
            map.remove(id);
        }
    }

    stale
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::mocks::*;

    #[test]
    fn should_evict_stale_senders() {
        let notifier = MockFrontendNotifier::new();
        let tracker = Mutex::new(HashMap::new());
        tracker.lock().unwrap().insert(
            DeviceId("stale".into()),
            Instant::now() - Duration::from_secs(60),
        );
        tracker
            .lock()
            .unwrap()
            .insert(DeviceId("fresh".into()), Instant::now());

        let evicted = evict_stale_senders(&notifier, &tracker, Duration::from_secs(30));

        assert_eq!(evicted.len(), 1);
        assert_eq!(evicted[0].0, "stale");

        let events = notifier.take_events();
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            FrontendEvent::SenderTimeout(id) if id.0 == "stale"
        ));

        // Fresh sender remains in tracker
        assert!(
            tracker
                .lock()
                .unwrap()
                .contains_key(&DeviceId("fresh".into()))
        );
        assert!(
            !tracker
                .lock()
                .unwrap()
                .contains_key(&DeviceId("stale".into()))
        );
    }

    #[test]
    fn should_not_evict_fresh_senders() {
        let notifier = MockFrontendNotifier::new();
        let tracker = Mutex::new(HashMap::new());
        tracker
            .lock()
            .unwrap()
            .insert(DeviceId("fresh".into()), Instant::now());

        let evicted = evict_stale_senders(&notifier, &tracker, Duration::from_secs(30));

        assert!(evicted.is_empty());
        assert!(notifier.take_events().is_empty());
    }

    #[test]
    fn should_evict_multiple_stale_senders() {
        let notifier = MockFrontendNotifier::new();
        let tracker = Mutex::new(HashMap::new());
        tracker.lock().unwrap().insert(
            DeviceId("s1".into()),
            Instant::now() - Duration::from_secs(60),
        );
        tracker.lock().unwrap().insert(
            DeviceId("s2".into()),
            Instant::now() - Duration::from_secs(45),
        );

        let evicted = evict_stale_senders(&notifier, &tracker, Duration::from_secs(30));

        assert_eq!(evicted.len(), 2);
        assert_eq!(notifier.take_events().len(), 2);
        assert!(tracker.lock().unwrap().is_empty());
    }
}
