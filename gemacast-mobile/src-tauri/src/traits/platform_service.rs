use gemacast_core::domain::types::DeviceId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackState {
    Playing,
    Paused,
    Stopped,
}

/// Whether the app may post the streaming notification.
///
/// Denial does not stop playback — `startForeground` needs no permission — but it
/// removes the only Pause and Disconnect controls that exist outside the app, so
/// the UI surfaces it rather than failing silently.
///
/// Mirrors `NotificationPermissionState` in `NotificationPermissionPolicy.kt`,
/// which is where the reasoning behind the [`Denied`](Self::Denied) /
/// [`Blocked`](Self::Blocked) split lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum NotificationPermission {
    /// Below Android 13, or not Android at all: nothing to request.
    NotRequired,
    Granted,
    /// Refused, but the system will still show its dialog if asked again.
    Denied,
    /// Refused for good. Only [`PlatformService::open_notification_settings`]
    /// can lead anywhere from here.
    Blocked,
}

impl NotificationPermission {
    /// Parse the wire value produced by `NotificationPermissionPolicy.wireValue`.
    pub fn from_wire(value: &str) -> Result<Self, String> {
        match value {
            "NOT_REQUIRED" => Ok(Self::NotRequired),
            "GRANTED" => Ok(Self::Granted),
            "DENIED" => Ok(Self::Denied),
            "BLOCKED" => Ok(Self::Blocked),
            other => Err(format!("unknown notification permission state: {other}")),
        }
    }
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

    /// Return the IDs of all PCs with a locally stored certificate pin.
    fn paired_pc_ids(&self) -> Result<Vec<DeviceId>, String>;

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

    /// Remove one PC certificate pin so the next connection requires phone
    /// confirmation again.
    fn forget_pc_identity(&self, pc_id: &DeviceId) -> Result<(), String>;

    /// Synchronize the Android foreground service state.
    fn sync_service(&self, state: PlaybackState, is_exclusive: bool);

    /// Set or clear the streaming-active flag file in the app cache directory.
    fn set_streaming_flag(&self, active: bool);

    /// Report whether the app may currently post the streaming notification.
    fn notification_permission(&self) -> Result<NotificationPermission, String>;

    /// Open this app's notification settings.
    ///
    /// The only recovery from [`NotificationPermission::Blocked`]: re-requesting
    /// the permission at that point shows the user nothing at all.
    fn open_notification_settings(&self) -> Result<(), String>;
}
