//! Periodically evicts devices that stop sending probe heartbeats.
//!
//! Devices connected over WiFi must send periodic probes to stay registered.
//! ADB (loopback) devices are exempt because their connection is managed
//! by the USB/ADB port-forwarding watchdog in `gemacast-core`.

use std::sync::Arc;
use std::time::Duration;

use tokio::task::JoinSet;

use crate::traits::{AudioController, DeviceRegistry, TrayNotifier};

/// How often the watchdog checks for stale devices.
const CHECK_INTERVAL: Duration = Duration::from_secs(2);

/// How long a device can go without a probe before being evicted.
const STALE_TIMEOUT: Duration = Duration::from_secs(10);

/// Evict devices whose `last_seen` exceeds `timeout` and notify the tray + audio engine.
///
/// This is the per-tick body of the watchdog, extracted as a standalone function
/// so it can be unit-tested without timers or task spawning.
pub async fn evict_stale_devices(
    registry: &dyn DeviceRegistry,
    tray: &dyn TrayNotifier,
    audio: &dyn AudioController,
    timeout: Duration,
) {
    let stale = registry.evict_stale(timeout);
    for (device_id, addr) in stale {
        tray.notify_device_lost(device_id.clone(), addr);
        audio.unsubscribe(&device_id).await;
    }
}

/// Spawn a watchdog task that periodically calls [`evict_stale_devices`].
pub fn spawn_device_watchdog(
    set: &mut JoinSet<()>,
    registry: Arc<dyn DeviceRegistry>,
    tray: Arc<dyn TrayNotifier>,
    audio: Arc<dyn AudioController>,
) {
    set.spawn(async move {
        let mut interval = tokio::time::interval(CHECK_INTERVAL);
        loop {
            interval.tick().await;
            evict_stale_devices(registry.as_ref(), tray.as_ref(), audio.as_ref(), STALE_TIMEOUT)
                .await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::mocks::{AudioCall, MockAudioController, MockDeviceRegistry, MockTrayNotifier, TrayCall};
    use std::time::Instant;

    #[tokio::test]
    async fn should_evict_device_older_than_timeout() {
        let registry = MockDeviceRegistry::new();
        registry.add_device_with_last_seen(
            "stale",
            "192.168.1.5:5000",
            Instant::now() - Duration::from_secs(15),
        );
        registry.add_device_with_last_seen("fresh", "192.168.1.6:5000", Instant::now());

        let tray = MockTrayNotifier::new();
        let audio = MockAudioController::new();

        evict_stale_devices(&registry, &tray, &audio, Duration::from_secs(10)).await;

        let tray_calls = tray.take_calls();
        assert_eq!(tray_calls.len(), 1);
        assert!(matches!(&tray_calls[0], TrayCall::Lost { device_id, .. } if device_id.0 == "stale"));

        let audio_calls = audio.take_calls();
        assert_eq!(audio_calls.len(), 1);
        assert!(matches!(&audio_calls[0], AudioCall::Unsubscribe { device_id } if device_id.0 == "stale"));

        assert!(registry.contains("fresh"));
        assert!(!registry.contains("stale"));
    }

    #[tokio::test]
    async fn should_not_evict_fresh_device() {
        let registry = MockDeviceRegistry::new();
        registry.add_device_with_last_seen("fresh", "192.168.1.6:5000", Instant::now());

        let tray = MockTrayNotifier::new();
        let audio = MockAudioController::new();

        evict_stale_devices(&registry, &tray, &audio, Duration::from_secs(10)).await;

        assert!(tray.take_calls().is_empty());
        assert!(audio.take_calls().is_empty());
        assert!(registry.contains("fresh"));
    }

    #[tokio::test]
    async fn should_not_evict_stale_loopback_device() {
        let registry = MockDeviceRegistry::new();
        registry.add_device_with_last_seen(
            "adb-dev",
            "127.0.0.1:5000",
            Instant::now() - Duration::from_secs(60),
        );

        let tray = MockTrayNotifier::new();
        let audio = MockAudioController::new();

        evict_stale_devices(&registry, &tray, &audio, Duration::from_secs(10)).await;

        assert!(tray.take_calls().is_empty());
        assert!(audio.take_calls().is_empty());
        assert!(registry.contains("adb-dev"));
    }
}
