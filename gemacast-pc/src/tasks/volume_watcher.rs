use std::sync::Arc;
use std::time::Duration;

use gemacast_core::control::types::WsEvent;
use gemacast_core::ports::output_volume::OutputVolumeReader;
use tokio::task::JoinSet;

use crate::adapters::device::WsConnectionMap;
use crate::traits::DeviceRegistry;

const POLL_INTERVAL: Duration = Duration::from_millis(250);

const EPSILON: f32 = 0.004;

pub fn is_meaningful_change(previous: Option<f32>, current: f32) -> bool {
    match previous {
        None => true,
        Some(previous) => (previous - current).abs() >= EPSILON,
    }
}

pub async fn poll_once(
    reader: &dyn OutputVolumeReader,
    registry: &dyn DeviceRegistry,
    ws_connections: &WsConnectionMap,
    last_sent: &mut Option<f32>,
) -> Option<f32> {
    if registry.all_devices().is_empty() {
        *last_sent = None;
        return None;
    }

    let level = reader.read()?;
    if !is_meaningful_change(*last_sent, level) {
        return None;
    }

    *last_sent = Some(level);
    gemacast_core::control::http::broadcast_ws_event(
        ws_connections,
        WsEvent::VolumeChanged { level },
    )
    .await;
    Some(level)
}

pub fn spawn_volume_watcher<R: OutputVolumeReader>(
    set: &mut JoinSet<()>,
    reader: R,
    registry: Arc<dyn DeviceRegistry>,
    ws_connections: WsConnectionMap,
) {
    set.spawn(async move {
        let mut ticker = tokio::time::interval(POLL_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut last_sent: Option<f32> = None;

        loop {
            ticker.tick().await;
            poll_once(&reader, registry.as_ref(), &ws_connections, &mut last_sent).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::mocks::MockDeviceRegistry;
    use gemacast_core::domain::types::DeviceId;
    use std::sync::Mutex;

    struct StubReader {
        levels: Mutex<Vec<Option<f32>>>,
    }

    impl StubReader {
        fn new(levels: Vec<Option<f32>>) -> Self {
            Self {
                levels: Mutex::new(levels),
            }
        }

        fn fixed(level: Option<f32>) -> Self {
            Self::new(vec![level])
        }
    }

    impl OutputVolumeReader for StubReader {
        fn read(&self) -> Option<f32> {
            let mut levels = self.levels.lock().unwrap();
            if levels.len() > 1 {
                levels.remove(0)
            } else {
                levels.first().copied().flatten()
            }
        }
    }

    fn registry_with_one_device() -> MockDeviceRegistry {
        MockDeviceRegistry::with_device("phone_1", "192.168.1.20:23556")
    }

    fn empty_connections() -> WsConnectionMap {
        Arc::new(std::sync::Mutex::new(Default::default()))
    }

    mod is_meaningful_change {
        use super::*;

        #[test]
        fn the_first_reading_is_always_meaningful() {
            assert!(is_meaningful_change(None, 0.0));
            assert!(is_meaningful_change(None, 0.5));
        }

        #[test]
        fn an_identical_reading_is_not_republished() {
            assert!(!is_meaningful_change(Some(0.5), 0.5));
        }

        #[test]
        fn a_change_below_the_epsilon_is_suppressed() {
            assert!(!is_meaningful_change(Some(0.5), 0.502));
        }

        #[test]
        fn a_change_at_or_above_the_epsilon_is_published() {
            assert!(is_meaningful_change(Some(0.5), 0.504));
            assert!(is_meaningful_change(Some(0.5), 0.6));
            assert!(is_meaningful_change(Some(0.5), 0.4));
        }
    }

    mod poll_once {
        use super::*;

        #[tokio::test]
        async fn nothing_is_read_while_no_device_is_subscribed() {
            let reader = StubReader::fixed(Some(0.7));
            let registry = MockDeviceRegistry::new();
            let mut last_sent = None;

            let sent = poll_once(&reader, &registry, &empty_connections(), &mut last_sent).await;

            assert_eq!(sent, None);
            assert_eq!(last_sent, None);
        }

        #[tokio::test]
        async fn the_first_reading_with_a_subscriber_is_published() {
            let reader = StubReader::fixed(Some(0.7));
            let registry = registry_with_one_device();
            let mut last_sent = None;

            let sent = poll_once(&reader, &registry, &empty_connections(), &mut last_sent).await;

            assert_eq!(sent, Some(0.7));
            assert_eq!(last_sent, Some(0.7));
        }

        #[tokio::test]
        async fn an_unchanged_level_is_not_republished_on_the_next_tick() {
            let reader = StubReader::fixed(Some(0.7));
            let registry = registry_with_one_device();
            let connections = empty_connections();
            let mut last_sent = None;

            let first = poll_once(&reader, &registry, &connections, &mut last_sent).await;
            let second = poll_once(&reader, &registry, &connections, &mut last_sent).await;

            assert_eq!(first, Some(0.7));
            assert_eq!(second, None);
        }

        #[tokio::test]
        async fn a_changed_level_is_published_after_a_steady_one() {
            let reader = StubReader::new(vec![Some(0.7), Some(0.7), Some(0.2)]);
            let registry = registry_with_one_device();
            let connections = empty_connections();
            let mut last_sent = None;

            assert_eq!(
                poll_once(&reader, &registry, &connections, &mut last_sent).await,
                Some(0.7)
            );
            assert_eq!(
                poll_once(&reader, &registry, &connections, &mut last_sent).await,
                None
            );
            assert_eq!(
                poll_once(&reader, &registry, &connections, &mut last_sent).await,
                Some(0.2)
            );
        }

        #[tokio::test]
        async fn an_unreadable_level_publishes_nothing_and_keeps_the_last_value() {
            let reader = StubReader::new(vec![Some(0.7), None]);
            let registry = registry_with_one_device();
            let connections = empty_connections();
            let mut last_sent = None;

            poll_once(&reader, &registry, &connections, &mut last_sent).await;
            let second = poll_once(&reader, &registry, &connections, &mut last_sent).await;

            assert_eq!(second, None);
            assert_eq!(last_sent, Some(0.7));
        }

        #[tokio::test]
        async fn losing_every_subscriber_clears_the_memo_so_a_rejoin_gets_the_level() {
            let reader = StubReader::fixed(Some(0.7));
            let registry = registry_with_one_device();
            let connections = empty_connections();
            let mut last_sent = None;

            poll_once(&reader, &registry, &connections, &mut last_sent).await;
            assert_eq!(last_sent, Some(0.7));

            registry.unregister(&DeviceId("phone_1".to_string()));
            poll_once(&reader, &registry, &connections, &mut last_sent).await;
            assert_eq!(last_sent, None);

            let rejoined = registry_with_one_device();
            let sent = poll_once(&reader, &rejoined, &connections, &mut last_sent).await;
            assert_eq!(sent, Some(0.7));
        }
    }
}
