use crate::traits::TrayNotifier;
use crate::updater::platform_key;
use std::sync::Arc;
use tokio::task::JoinSet;

/// Spawns the background update checker.
///
/// Wait 3 seconds at startup, then check `gemacast_core::updater` for a new release.
/// If one exists, download it silently in the background and notify the tray.
pub fn spawn_update_checker(set: &mut JoinSet<()>, tray: Arc<dyn TrayNotifier>) {
    set.spawn(async move {
        // Small delay so the tray is fully initialised before we touch it.
        tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;

        let current_version = env!("CARGO_PKG_VERSION");
        let key = match platform_key() {
            Some(k) => k,
            None => {
                tracing::info!(
                    "Auto-updates are not supported on this platform/architecture triple."
                );
                return;
            }
        };

        let info = match gemacast_core::updater::check_for_update(current_version, key).await {
            Ok(Some(info)) => info,
            Ok(None) => return,
            Err(e) => {
                tracing::warn!("Update check failed: {}", e);
                tray.notify_update_failed(e);
                return;
            }
        };

        // Streamlined UX: download silently in the background.
        // NOTE: we don't pass a progress channel since the PC tray has no download progress UI.

        // Derive a filename from the URL.
        let filename = info
            .download_url
            .rsplit('/')
            .next()
            .unwrap_or("gemacast-update");

        let dir = std::env::temp_dir().join("gemacast-update");
        let _ = std::fs::create_dir_all(&dir);
        let file_path = dir.join(filename);

        match gemacast_core::updater::download_update(&info.download_url, &file_path, None).await {
            Ok(()) => {
                tray.notify_update_ready(info.version, file_path);
            }
            Err(e) => {
                tracing::warn!("Update download failed: {}", e);
                tray.notify_update_failed(e);
            }
        }
    });
}
