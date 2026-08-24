use gemacast_core::domain::types::{DeviceId, DiscoveredDevice};
use std::net::SocketAddr;
use std::time::Duration;

/// Outcome of a [`DeviceRegistry::register`] call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistrationOutcome {
    /// The device was not previously known.
    NewDevice,
    /// The device was known but its address changed.
    AddressChanged { old_addr: SocketAddr },
    /// The device was already registered at the same address.
    AlreadyRegistered,
}

/// Shared registry of connected player devices.
///
/// Encapsulates all lock-and-mutate operations on the device map.
///
/// **Production**: [`crate::state::SharedMapDeviceRegistry`]
/// **Tests**: [`crate::testing::mocks::MockDeviceRegistry`]
pub trait DeviceRegistry: Send + Sync {
    /// Insert or update a device, returning what changed.
    fn register(&self, device: DiscoveredDevice) -> RegistrationOutcome;

    /// Remove a device, returning it if it existed.
    fn unregister(&self, device_id: &DeviceId) -> Option<DiscoveredDevice>;

    /// Refresh the `last_seen` timestamp for a device (used by probe heartbeats).
    ///
    /// Returns whether the device was found — i.e. whether it is still
    /// registered. A probe must never *resurrect* an evicted device, so this
    /// only refreshes an existing entry, and the boolean is therefore a truthful
    /// liveness answer rather than a self-fulfilling one. The probe response
    /// carries it back to the phone as `device_registered`.
    fn update_last_seen(&self, device_id: &DeviceId) -> bool;

    /// Get the address of a device, if registered.
    fn get_addr(&self, device_id: &DeviceId) -> Option<SocketAddr>;

    /// Snapshot of all registered devices.
    fn all_devices(&self) -> Vec<(DeviceId, DiscoveredDevice)>;

    /// Remove and return all devices.
    fn drain_all(&self) -> Vec<(DeviceId, DiscoveredDevice)>;

    /// Remove devices whose `last_seen` exceeds `timeout`, including ADB devices.
    /// Returns the evicted `(device_id, addr)` pairs.
    fn evict_stale(&self, timeout: Duration) -> Vec<(DeviceId, SocketAddr)>;
}
