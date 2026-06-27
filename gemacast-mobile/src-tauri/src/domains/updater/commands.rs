use gemacast_core::updater::UpdateInfo;
use tauri::{Emitter, Manager};
use tokio::sync::mpsc;

/// Check whether an update is available.
///
/// Returns `Some(UpdateInfo)` when a newer version exists, or `None` when
/// the app is already up-to-date.
#[tauri::command]
pub async fn check_for_update(app: tauri::AppHandle) -> Result<Option<UpdateInfo>, String> {
    let current_version = app.config().version.clone().unwrap_or_default();
    gemacast_core::updater::check_for_update(&current_version, "android").await
}

/// Download the update APK to the app's cache directory.
///
/// Emits `update-progress` events to the frontend with the download percentage.
/// Returns the absolute path to the downloaded APK file.
#[tauri::command]
pub async fn download_update(app: tauri::AppHandle, url: String) -> Result<String, String> {
    let cache_dir = app
        .path()
        .app_cache_dir()
        .map_err(|e| format!("Failed to get cache dir: {e}"))?
        .join("updates");

    std::fs::create_dir_all(&cache_dir)
        .map_err(|e| format!("Failed to create updates dir: {e}"))?;

    let file_path = cache_dir.join("gemacast-update.apk");

    let (progress_tx, mut progress_rx) = mpsc::channel::<u8>(32);

    let app_handle = app.clone();
    tokio::spawn(async move {
        while let Some(percent) = progress_rx.recv().await {
            let _ = app_handle.emit("update-progress", percent);
        }
    });

    gemacast_core::updater::download_update(&url, &file_path, Some(progress_tx)).await?;

    file_path
        .to_str()
        .map(String::from)
        .ok_or_else(|| "Path contains invalid UTF-8".to_string())
}

/// Trigger the Android system installer for the downloaded APK.
///
/// On non-Android platforms this is a no-op that returns an error.
#[tauri::command]
pub async fn install_apk(app: tauri::AppHandle, path: String) -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        crate::domains::updater::install::install_apk_android(&app, &path)
    }

    #[cfg(not(target_os = "android"))]
    {
        let _ = (app, path);
        Err("APK installation is only supported on Android".to_string())
    }
}
