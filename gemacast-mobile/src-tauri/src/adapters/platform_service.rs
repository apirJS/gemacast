use crate::traits::{NotificationPermission, PlatformService, PlaybackState};
use std::sync::Arc;

#[cfg(target_os = "android")]
use base64::Engine;

/// Platform-specific operations backed by the real OS and Tauri APIs.
pub struct NativePlatformService {
    app_handle: tauri::AppHandle,
}

impl NativePlatformService {
    pub fn new(app_handle: tauri::AppHandle) -> Self {
        Self { app_handle }
    }
}

impl PlatformService for NativePlatformService {
    fn get_transport_type(&self) -> Result<String, String> {
        #[cfg(target_os = "android")]
        {
            crate::domains::discovery::native::call_native_transport_check(&self.app_handle)
        }
        #[cfg(not(target_os = "android"))]
        {
            Err("Not supported on this platform".to_string())
        }
    }

    fn device_public_key(&self) -> Result<String, String> {
        #[cfg(target_os = "android")]
        {
            crate::domains::discovery::native::call_native_device_public_key(&self.app_handle)
        }
        #[cfg(not(target_os = "android"))]
        {
            Err("Device identity requires Android Keystore".to_string())
        }
    }

    fn sign_device_auth(&self, transcript: &[u8]) -> Result<String, String> {
        #[cfg(target_os = "android")]
        {
            let transcript = base64::engine::general_purpose::STANDARD.encode(transcript);
            crate::domains::discovery::native::call_native_sign_device_auth(
                &self.app_handle,
                &transcript,
            )
        }
        #[cfg(not(target_os = "android"))]
        {
            let _ = transcript;
            Err("Device identity requires Android Keystore".to_string())
        }
    }

    fn trusted_pc_fingerprint(
        &self,
        pc_id: &gemacast_core::domain::types::DeviceId,
    ) -> Result<Option<String>, String> {
        #[cfg(target_os = "android")]
        {
            crate::domains::discovery::native::call_native_trusted_pc_fingerprint(
                &self.app_handle,
                pc_id.as_ref(),
            )
        }
        #[cfg(not(target_os = "android"))]
        {
            let _ = pc_id;
            Ok(None)
        }
    }

    fn paired_pc_ids(&self) -> Result<Vec<gemacast_core::domain::types::DeviceId>, String> {
        #[cfg(target_os = "android")]
        {
            crate::domains::discovery::native::call_native_paired_pc_ids(&self.app_handle)
        }
        #[cfg(not(target_os = "android"))]
        {
            Ok(Vec::new())
        }
    }

    fn confirm_pc_identity(
        &self,
        pc_id: &gemacast_core::domain::types::DeviceId,
        pc_name: &str,
        fingerprint: &str,
        pairing_code: &str,
        requires_approval: bool,
    ) -> Result<bool, String> {
        #[cfg(target_os = "android")]
        {
            crate::domains::discovery::native::call_native_confirm_pc_identity(
                &self.app_handle,
                pc_id.as_ref(),
                pc_name,
                fingerprint,
                pairing_code,
                requires_approval,
            )
        }
        #[cfg(not(target_os = "android"))]
        {
            let _ = (pc_id, pc_name, fingerprint, pairing_code, requires_approval);
            Ok(true)
        }
    }

    fn remember_pc_identity(
        &self,
        pc_id: &gemacast_core::domain::types::DeviceId,
        fingerprint: &str,
    ) -> Result<(), String> {
        #[cfg(target_os = "android")]
        {
            crate::domains::discovery::native::call_native_remember_pc_identity(
                &self.app_handle,
                pc_id.as_ref(),
                fingerprint,
            )
        }
        #[cfg(not(target_os = "android"))]
        {
            let _ = (pc_id, fingerprint);
            Ok(())
        }
    }

    fn forget_pc_identity(
        &self,
        pc_id: &gemacast_core::domain::types::DeviceId,
    ) -> Result<(), String> {
        #[cfg(target_os = "android")]
        {
            crate::domains::discovery::native::call_native_forget_pc_identity(
                &self.app_handle,
                pc_id.as_ref(),
            )
        }
        #[cfg(not(target_os = "android"))]
        {
            let _ = pc_id;
            Ok(())
        }
    }

    #[allow(unused_variables)]
    fn sync_service(&self, state: PlaybackState, is_exclusive: bool) {
        #[cfg(target_os = "android")]
        {
            let action = match state {
                PlaybackState::Playing => "SYNC_PLAYING",
                PlaybackState::Paused => "SYNC_PAUSED",
                PlaybackState::Stopped => "SYNC_STOPPED",
            };
            let _ = crate::domains::discovery::native::call_native_sync_service(
                &self.app_handle,
                action,
                is_exclusive,
            );
        }
    }

    fn set_streaming_flag(&self, active: bool) {
        use tauri::Manager;
        if let Ok(cache_dir) = self.app_handle.path().app_cache_dir() {
            let flag_path = cache_dir.join(".streaming_active");
            if active {
                let _ = std::fs::create_dir_all(&cache_dir);
                let _ = std::fs::write(&flag_path, "1");
            } else {
                let _ = std::fs::remove_file(&flag_path);
            }
        }
    }

    fn notification_permission(&self) -> Result<NotificationPermission, String> {
        #[cfg(target_os = "android")]
        {
            let wire =
                crate::domains::discovery::native::call_native_notification_permission_state(
                    &self.app_handle,
                )?;
            NotificationPermission::from_wire(&wire)
        }
        #[cfg(not(target_os = "android"))]
        {
            Ok(NotificationPermission::NotRequired)
        }
    }

    fn open_notification_settings(&self) -> Result<(), String> {
        #[cfg(target_os = "android")]
        {
            crate::domains::discovery::native::call_native_open_notification_settings(
                &self.app_handle,
            )
        }
        #[cfg(not(target_os = "android"))]
        {
            Err("Notification settings are only available on Android".to_string())
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

impl gemacast_core::control::http_client::DeviceAuthSigner for PlatformDeviceAuthSigner {
    fn public_key(&self) -> Result<String, String> {
        self.platform.device_public_key()
    }

    fn sign(&self, transcript: &[u8]) -> Result<String, String> {
        self.platform.sign_device_auth(transcript)
    }

    fn trusted_pc_fingerprint(
        &self,
        pc_id: &gemacast_core::domain::types::DeviceId,
    ) -> Result<Option<String>, String> {
        self.platform.trusted_pc_fingerprint(pc_id)
    }

    fn confirm_pc_identity(
        &self,
        pc_id: &gemacast_core::domain::types::DeviceId,
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

    fn remember_pc_identity(
        &self,
        pc_id: &gemacast_core::domain::types::DeviceId,
        fingerprint: &str,
    ) -> Result<(), String> {
        self.platform.remember_pc_identity(pc_id, fingerprint)
    }
}
