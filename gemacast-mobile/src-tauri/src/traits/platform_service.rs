use gemacast_core::domain::types::DeviceId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackState {
    Playing,
    Paused,
    Stopped,
}

/// Platform-specific operations (Android JNI, foreground service, file I/O).
///
/// **Production**: [`crate::adapters::NativePlatformService`]
/// **Tests**: [`crate::testing::mocks::MockPlatformService`]
pub trait PlatformService: Send + Sync {
    /// Get the active transport type string (e.g. `"WIFI|ADB_ON"`).
    ///
    /// Returns `Err` on non-Android platforms or if JNI fails.
    fn get_transport_type(&self) -> Result<String, String>;

    /// Return the Android Keystore-backed P-256 public key.
    fn device_public_key(&self) -> Result<String, String>;

    /// Sign one device-authentication transcript without exporting the key.
    fn sign_device_auth(&self, transcript: &[u8]) -> Result<String, String>;

    /// Return the app-private certificate pin for a previously approved PC.
    fn trusted_pc_fingerprint(&self, pc_id: &DeviceId) -> Result<Option<String>, String>;

    /// Ask the phone user to compare the PC's pairing code.
    fn confirm_pc_identity(
        &self,
        pc_id: &DeviceId,
        pc_name: &str,
        fingerprint: &str,
        pairing_code: &str,
        requires_approval: bool,
    ) -> Result<bool, String>;

    /// Persist the staged PC certificate pin after pairing and stream startup.
    fn remember_pc_identity(&self, pc_id: &DeviceId, fingerprint: &str) -> Result<(), String>;

    /// Synchronize the Android foreground service state.
    fn sync_service(&self, state: PlaybackState, is_exclusive: bool);

    /// Set or clear the streaming-active flag file in the app cache directory.
    fn set_streaming_flag(&self, active: bool);
}
