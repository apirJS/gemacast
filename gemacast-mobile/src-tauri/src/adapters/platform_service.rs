use crate::traits::{PlatformService, PlaybackState};

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

    #[allow(unused_variables)]
    fn sync_service(&self, state: PlaybackState, is_exclusive: bool) {
        #[cfg(target_os = "android")]
        {
            let action = match state {
                PlaybackState::Playing => "START",
                PlaybackState::Paused => "STOP_STREAM",
                PlaybackState::Stopped => "DISCONNECT",
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
}
