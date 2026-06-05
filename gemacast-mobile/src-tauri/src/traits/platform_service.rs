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

    /// Synchronize the Android foreground service state.
    fn sync_service(&self, state: PlaybackState, is_exclusive: bool);

    /// Set or clear the streaming-active flag file in the app cache directory.
    fn set_streaming_flag(&self, active: bool);
}
