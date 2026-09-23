use std::sync::Arc;

use gemacast_core::domain::types::DeviceId;

use crate::services::platform::PlatformFacade;
use crate::traits::{PlatformService, PlaybackState};

pub struct NativePlatformService {
    platform: PlatformFacade,
}

impl NativePlatformService {
    pub fn new(app_handle: tauri::AppHandle) -> Self {
        Self {
            platform: PlatformFacade::new(app_handle),
        }
    }
}

impl PlatformService for NativePlatformService {
    fn get_transport_type(&self) -> Result<String, String> {
        self.platform
            .transport_type()
            .map_err(|error| error.to_string())
    }

    fn device_public_key(&self) -> Result<String, String> {
        self.platform
            .device_public_key()
            .map_err(|error| error.to_string())
    }

    fn sign_device_auth(&self, transcript: &[u8]) -> Result<String, String> {
        self.platform
            .sign_device_auth(transcript)
            .map_err(|error| error.to_string())
    }

    fn trusted_pc_fingerprint(&self, pc_id: &DeviceId) -> Result<Option<String>, String> {
        self.platform
            .trusted_pc_fingerprint(pc_id)
            .map_err(|error| error.to_string())
    }

    fn paired_pc_ids(&self) -> Result<Vec<DeviceId>, String> {
        self.platform
            .paired_pc_ids()
            .map_err(|error| error.to_string())
    }

    fn confirm_pc_identity(
        &self,
        pc_id: &DeviceId,
        pc_name: &str,
        fingerprint: &str,
        pairing_code: &str,
        requires_approval: bool,
    ) -> Result<bool, String> {
        self.platform
            .confirm_pc_identity(pc_id, pc_name, fingerprint, pairing_code, requires_approval)
            .map_err(|error| error.to_string())
    }

    fn remember_pc_identity(&self, pc_id: &DeviceId, fingerprint: &str) -> Result<(), String> {
        self.platform
            .remember_pc_identity(pc_id, fingerprint)
            .map_err(|error| error.to_string())
    }

    fn forget_pc_identity(&self, pc_id: &DeviceId) -> Result<(), String> {
        self.platform
            .forget_pc_identity(pc_id)
            .map_err(|error| error.to_string())
    }

    fn sync_service(&self, state: PlaybackState, is_exclusive: bool) {
        if let Err(error) = self.platform.sync_service(state, is_exclusive) {
            tracing::warn!("could not synchronize the platform service: {error}");
        }
    }

    fn set_streaming_flag(&self, active: bool) {
        if let Err(error) = self.platform.set_streaming_flag(active) {
            tracing::warn!("could not update the streaming state: {error}");
        }
    }
}

pub struct PlatformDeviceAuthSigner {
    platform: Arc<dyn PlatformService>,
}

impl PlatformDeviceAuthSigner {
    pub fn new(platform: Arc<dyn PlatformService>) -> Self {
        Self { platform }
    }
}

impl gemacast_core::control::DeviceAuthSigner for PlatformDeviceAuthSigner {
    fn public_key(&self) -> Result<String, String> {
        self.platform.device_public_key()
    }

    fn sign(&self, transcript: &[u8]) -> Result<String, String> {
        self.platform.sign_device_auth(transcript)
    }

    fn trusted_pc_fingerprint(&self, pc_id: &DeviceId) -> Result<Option<String>, String> {
        self.platform.trusted_pc_fingerprint(pc_id)
    }

    fn confirm_pc_identity(
        &self,
        pc_id: &DeviceId,
        pc_name: &str,
        fingerprint: &str,
        pairing_code: &str,
        requires_approval: bool,
    ) -> Result<bool, String> {
        self.platform.confirm_pc_identity(
            pc_id,
            pc_name,
            fingerprint,
            pairing_code,
            requires_approval,
        )
    }

    fn remember_pc_identity(&self, pc_id: &DeviceId, fingerprint: &str) -> Result<(), String> {
        self.platform.remember_pc_identity(pc_id, fingerprint)
    }
}
