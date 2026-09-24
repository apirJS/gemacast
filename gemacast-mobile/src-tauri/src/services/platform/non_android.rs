use gemacast_core::domain::types::DeviceId;

use crate::services::error::{ServiceError, ServiceResult};
use crate::traits::PlaybackState;

pub(super) struct NonAndroidPlatform;

impl NonAndroidPlatform {
    pub(super) fn new(_app_handle: tauri::AppHandle) -> Self {
        Self
    }

    pub(super) fn transport_type(&self) -> ServiceResult<String> {
        Err(Self::unsupported("transport detection"))
    }

    pub(super) fn device_public_key(&self) -> ServiceResult<String> {
        Err(Self::unsupported("device identity"))
    }

    pub(super) fn sign_device_auth(&self, _transcript: &[u8]) -> ServiceResult<String> {
        Err(Self::unsupported("device identity"))
    }

    pub(super) fn trusted_pc_fingerprint(
        &self,
        _pc_id: &DeviceId,
    ) -> ServiceResult<Option<String>> {
        Ok(None)
    }

    pub(super) fn paired_pc_ids(&self) -> ServiceResult<Vec<DeviceId>> {
        Ok(Vec::new())
    }

    pub(super) fn confirm_pc_identity(
        &self,
        _pc_id: &DeviceId,
        _pc_name: &str,
        _fingerprint: &str,
        _pairing_code: &str,
        _requires_approval: bool,
    ) -> ServiceResult<bool> {
        Ok(true)
    }

    pub(super) fn remember_pc_identity(
        &self,
        _pc_id: &DeviceId,
        _fingerprint: &str,
    ) -> ServiceResult<()> {
        Ok(())
    }

    pub(super) fn forget_pc_identity(&self, _pc_id: &DeviceId) -> ServiceResult<()> {
        Ok(())
    }

    pub(super) fn sync_service(
        &self,
        _state: PlaybackState,
        _is_exclusive: bool,
    ) -> ServiceResult<()> {
        Ok(())
    }

    pub(super) fn finish_and_remove_task(&self) -> ServiceResult<()> {
        Ok(())
    }

    pub(super) fn install_apk(&self, _path: &str) -> ServiceResult<()> {
        Err(Self::unsupported("APK installation"))
    }

    fn unsupported(operation: &'static str) -> ServiceError {
        ServiceError::UnsupportedPlatform { operation }
    }
}
