use base64::Engine;
use gemacast_core::domain::types::DeviceId;

use crate::services::error::{ServiceError, ServiceResult};
use crate::traits::PlaybackState;

pub(super) struct AndroidPlatform {
    app_handle: tauri::AppHandle,
}

impl AndroidPlatform {
    pub(super) fn new(app_handle: tauri::AppHandle) -> Self {
        Self { app_handle }
    }

    pub(super) fn transport_type(&self) -> ServiceResult<String> {
        crate::services::discovery::native::call_native_transport_check(&self.app_handle)
            .map_err(ServiceError::PlatformBridge)
    }

    pub(super) fn device_public_key(&self) -> ServiceResult<String> {
        crate::services::discovery::native::call_native_device_public_key(&self.app_handle)
            .map_err(ServiceError::PlatformBridge)
    }

    pub(super) fn sign_device_auth(&self, transcript: &[u8]) -> ServiceResult<String> {
        let transcript = base64::engine::general_purpose::STANDARD.encode(transcript);
        crate::services::discovery::native::call_native_sign_device_auth(
            &self.app_handle,
            &transcript,
        )
        .map_err(ServiceError::PlatformBridge)
    }

    pub(super) fn trusted_pc_fingerprint(&self, pc_id: &DeviceId) -> ServiceResult<Option<String>> {
        crate::services::discovery::native::call_native_trusted_pc_fingerprint(
            &self.app_handle,
            pc_id.as_ref(),
        )
        .map_err(ServiceError::PlatformBridge)
    }

    pub(super) fn paired_pc_ids(&self) -> ServiceResult<Vec<DeviceId>> {
        crate::services::discovery::native::call_native_paired_pc_ids(&self.app_handle)
            .map_err(ServiceError::PlatformBridge)
    }

    pub(super) fn confirm_pc_identity(
        &self,
        pc_id: &DeviceId,
        pc_name: &str,
        fingerprint: &str,
        pairing_code: &str,
        requires_approval: bool,
    ) -> ServiceResult<bool> {
        crate::services::discovery::native::call_native_confirm_pc_identity(
            &self.app_handle,
            pc_id.as_ref(),
            pc_name,
            fingerprint,
            pairing_code,
            requires_approval,
        )
        .map_err(ServiceError::PlatformBridge)
    }

    pub(super) fn remember_pc_identity(
        &self,
        pc_id: &DeviceId,
        fingerprint: &str,
    ) -> ServiceResult<()> {
        crate::services::discovery::native::call_native_remember_pc_identity(
            &self.app_handle,
            pc_id.as_ref(),
            fingerprint,
        )
        .map_err(ServiceError::PlatformBridge)
    }

    pub(super) fn forget_pc_identity(&self, pc_id: &DeviceId) -> ServiceResult<()> {
        crate::services::discovery::native::call_native_forget_pc_identity(
            &self.app_handle,
            pc_id.as_ref(),
        )
        .map_err(ServiceError::PlatformBridge)
    }

    pub(super) fn sync_service(
        &self,
        state: PlaybackState,
        is_exclusive: bool,
    ) -> ServiceResult<()> {
        let action = match state {
            PlaybackState::Playing => "SYNC_PLAYING",
            PlaybackState::Paused => "SYNC_PAUSED",
            PlaybackState::Stopped => "SYNC_STOPPED",
        };
        crate::services::discovery::native::call_native_sync_service(
            &self.app_handle,
            action,
            is_exclusive,
        )
        .map_err(ServiceError::PlatformBridge)
    }

    pub(super) fn finish_and_remove_task(&self) -> ServiceResult<()> {
        crate::services::discovery::native::call_native_finish_and_remove_task(&self.app_handle)
            .map_err(ServiceError::PlatformBridge)
    }

    pub(super) fn install_apk(&self, path: &str) -> ServiceResult<()> {
        crate::services::updater::install::install_apk_android(&self.app_handle, path)
            .map_err(ServiceError::PlatformBridge)
    }
}
