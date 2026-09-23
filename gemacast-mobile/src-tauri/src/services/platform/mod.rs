#[cfg(target_os = "android")]
mod android;
#[cfg(not(target_os = "android"))]
mod non_android;

use gemacast_core::domain::types::DeviceId;
use tauri::Manager;

use crate::services::error::{ServiceError, ServiceResult};
use crate::traits::PlaybackState;

#[cfg(target_os = "android")]
use android::AndroidPlatform as ActivePlatform;
#[cfg(not(target_os = "android"))]
use non_android::NonAndroidPlatform as ActivePlatform;

pub struct PlatformFacade {
    app_handle: tauri::AppHandle,
    platform: ActivePlatform,
}

impl PlatformFacade {
    pub fn new(app_handle: tauri::AppHandle) -> Self {
        let platform = ActivePlatform::new(app_handle.clone());
        Self {
            app_handle,
            platform,
        }
    }

    pub fn transport_type(&self) -> ServiceResult<String> {
        self.platform.transport_type()
    }

    pub fn device_public_key(&self) -> ServiceResult<String> {
        self.platform.device_public_key()
    }

    pub fn sign_device_auth(&self, transcript: &[u8]) -> ServiceResult<String> {
        self.platform.sign_device_auth(transcript)
    }

    pub fn trusted_pc_fingerprint(&self, pc_id: &DeviceId) -> ServiceResult<Option<String>> {
        self.platform.trusted_pc_fingerprint(pc_id)
    }

    pub fn paired_pc_ids(&self) -> ServiceResult<Vec<DeviceId>> {
        self.platform.paired_pc_ids()
    }

    pub fn confirm_pc_identity(
        &self,
        pc_id: &DeviceId,
        pc_name: &str,
        fingerprint: &str,
        pairing_code: &str,
        requires_approval: bool,
    ) -> ServiceResult<bool> {
        self.platform.confirm_pc_identity(
            pc_id,
            pc_name,
            fingerprint,
            pairing_code,
            requires_approval,
        )
    }

    pub fn remember_pc_identity(&self, pc_id: &DeviceId, fingerprint: &str) -> ServiceResult<()> {
        self.platform.remember_pc_identity(pc_id, fingerprint)
    }

    pub fn forget_pc_identity(&self, pc_id: &DeviceId) -> ServiceResult<()> {
        self.platform.forget_pc_identity(pc_id)
    }

    pub fn sync_service(&self, state: PlaybackState, is_exclusive: bool) -> ServiceResult<()> {
        self.platform.sync_service(state, is_exclusive)
    }

    pub fn finish_and_remove_task(&self) -> ServiceResult<()> {
        self.platform.finish_and_remove_task()
    }

    pub fn install_apk(&self, path: &str) -> ServiceResult<()> {
        self.platform.install_apk(path)
    }

    pub fn set_streaming_flag(&self, active: bool) -> ServiceResult<()> {
        let cache_dir = self
            .app_handle
            .path()
            .app_cache_dir()
            .map_err(|error| ServiceError::PlatformBridge(error.to_string()))?;
        let flag_path = cache_dir.join(".streaming_active");

        if active {
            std::fs::create_dir_all(&cache_dir).map_err(|source| ServiceError::FileSystem {
                operation: "create the app cache directory",
                source,
            })?;
            std::fs::write(flag_path, "1").map_err(|source| ServiceError::FileSystem {
                operation: "write the streaming state",
                source,
            })
        } else {
            match std::fs::remove_file(flag_path) {
                Ok(()) => Ok(()),
                Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(source) => Err(ServiceError::FileSystem {
                    operation: "clear the streaming state",
                    source,
                }),
            }
        }
    }
}
