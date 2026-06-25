//! Shared device registry — the central source of truth for connected devices.
//!
//! [`SharedMapDeviceRegistry`] wraps `Arc<Mutex<HashMap<DeviceId, DiscoveredDevice>>>`
//! and implements [`DeviceRegistry`](crate::traits::DeviceRegistry) so all lock-and-mutate
//! operations are encapsulated behind a testable trait.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gemacast_core::domain::types::{DeviceId, DiscoveredDevice};

use crate::traits::{DeviceRegistry, RegistrationOutcome};

/// Thread-safe device registry backed by `Arc<Mutex<HashMap>>`.
///
/// Created once at startup and shared (via `Arc<dyn DeviceRegistry>`) with all
/// background tasks that need to read or modify the device list.
#[derive(Clone)]
pub struct SharedMapDeviceRegistry {
    inner: Arc<Mutex<HashMap<DeviceId, DiscoveredDevice>>>,
}

impl SharedMapDeviceRegistry {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl DeviceRegistry for SharedMapDeviceRegistry {
    fn register(&self, device: DiscoveredDevice) -> RegistrationOutcome {
        let Ok(mut map) = self.inner.lock() else {
            return RegistrationOutcome::AlreadyRegistered;
        };

        let outcome = if let Some(existing) = map.get(&device.device_id) {
            if existing.addr != device.addr {
                RegistrationOutcome::AddressChanged {
                    old_addr: existing.addr,
                }
            } else {
                RegistrationOutcome::AlreadyRegistered
            }
        } else {
            RegistrationOutcome::NewDevice
        };

        map.insert(device.device_id.clone(), device);
        outcome
    }

    fn unregister(&self, device_id: &DeviceId) -> Option<DiscoveredDevice> {
        self.inner.lock().ok()?.remove(device_id)
    }

    fn update_last_seen(&self, device_id: &DeviceId) {
        if let Ok(mut map) = self.inner.lock()
            && let Some(device) = map.get_mut(device_id)
        {
            device.last_seen = std::time::Instant::now();
        }
    }

    fn get_addr(&self, device_id: &DeviceId) -> Option<SocketAddr> {
        self.inner.lock().ok()?.get(device_id).map(|d| d.addr)
    }

    fn all_devices(&self) -> Vec<(DeviceId, DiscoveredDevice)> {
        self.inner
            .lock()
            .ok()
            .map(|map| map.iter().map(|(id, d)| (id.clone(), d.clone())).collect())
            .unwrap_or_default()
    }

    fn drain_all(&self) -> Vec<(DeviceId, DiscoveredDevice)> {
        self.inner
            .lock()
            .ok()
            .map(|mut map| map.drain().collect())
            .unwrap_or_default()
    }

    fn evict_stale(&self, timeout: Duration) -> Vec<(DeviceId, SocketAddr)> {
        let Ok(mut map) = self.inner.lock() else {
            return Vec::new();
        };
        let now = std::time::Instant::now();
        let mut evicted = Vec::new();
        map.retain(|id, device| {
            if now.duration_since(device.last_seen) > timeout && !device.addr.ip().is_loopback() {
                evicted.push((id.clone(), device.addr));
                false
            } else {
                true
            }
        });
        evicted
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn make_device(id: &str, addr: &str) -> DiscoveredDevice {
        DiscoveredDevice::from_presence(
            DeviceId(id.to_string()),
            id.to_string(),
            false,
            addr.parse().unwrap(),
            None,
        )
    }

    #[test]
    fn register_should_return_new_device_for_first_registration() {
        let registry = SharedMapDeviceRegistry::new();
        let device = make_device("dev-1", "192.168.1.1:5000");

        let outcome = registry.register(device);

        assert_eq!(outcome, RegistrationOutcome::NewDevice);
    }

    #[test]
    fn register_should_return_already_registered_for_same_addr() {
        let registry = SharedMapDeviceRegistry::new();
        let device = make_device("dev-1", "192.168.1.1:5000");
        registry.register(device.clone());

        let outcome = registry.register(device);

        assert_eq!(outcome, RegistrationOutcome::AlreadyRegistered);
    }

    #[test]
    fn register_should_return_address_changed_when_addr_differs() {
        let registry = SharedMapDeviceRegistry::new();
        let device_v1 = make_device("dev-1", "192.168.1.1:5000");
        registry.register(device_v1);

        let device_v2 = make_device("dev-1", "192.168.1.2:5000");
        let outcome = registry.register(device_v2);

        assert_eq!(
            outcome,
            RegistrationOutcome::AddressChanged {
                old_addr: "192.168.1.1:5000".parse().unwrap()
            }
        );
    }

    #[test]
    fn unregister_should_return_removed_device() {
        let registry = SharedMapDeviceRegistry::new();
        let device = make_device("dev-1", "192.168.1.1:5000");
        registry.register(device);

        let removed = registry.unregister(&DeviceId("dev-1".to_string()));

        assert!(removed.is_some());
        assert_eq!(removed.unwrap().device_id.0, "dev-1");
    }

    #[test]
    fn unregister_should_return_none_for_unknown_device() {
        let registry = SharedMapDeviceRegistry::new();

        let removed = registry.unregister(&DeviceId("unknown".to_string()));

        assert!(removed.is_none());
    }

    #[test]
    fn evict_stale_should_remove_devices_older_than_timeout() {
        let registry = SharedMapDeviceRegistry::new();
        let mut stale = make_device("stale-dev", "192.168.1.5:5000");
        stale.last_seen = Instant::now() - Duration::from_secs(15);
        registry
            .inner
            .lock()
            .unwrap()
            .insert(stale.device_id.clone(), stale);

        let fresh = make_device("fresh-dev", "192.168.1.6:5000");
        registry.register(fresh);

        let evicted = registry.evict_stale(Duration::from_secs(10));

        assert_eq!(evicted.len(), 1);
        assert_eq!(evicted[0].0.0, "stale-dev");
        assert!(
            registry
                .get_addr(&DeviceId("fresh-dev".to_string()))
                .is_some()
        );
    }

    #[test]
    fn evict_stale_should_skip_loopback_devices() {
        let registry = SharedMapDeviceRegistry::new();
        let mut loopback = make_device("adb-dev", "127.0.0.1:5000");
        loopback.last_seen = Instant::now() - Duration::from_secs(60);
        registry
            .inner
            .lock()
            .unwrap()
            .insert(loopback.device_id.clone(), loopback);

        let evicted = registry.evict_stale(Duration::from_secs(10));

        assert!(evicted.is_empty());
        assert!(
            registry
                .get_addr(&DeviceId("adb-dev".to_string()))
                .is_some()
        );
    }

    #[test]
    fn drain_all_should_empty_registry() {
        let registry = SharedMapDeviceRegistry::new();
        registry.register(make_device("dev-1", "192.168.1.1:5000"));
        registry.register(make_device("dev-2", "192.168.1.2:5000"));

        let drained = registry.drain_all();

        assert_eq!(drained.len(), 2);
        assert!(registry.all_devices().is_empty());
    }
}
