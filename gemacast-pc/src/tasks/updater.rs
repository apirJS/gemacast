use crate::traits::TrayNotifier;
use crate::updater::platform_key;
use std::sync::Arc;
use tokio::task::JoinSet;

/// How often to re-check for updates (4 hours).
const RECHECK_INTERVAL: tokio::time::Duration = tokio::time::Duration::from_secs(4 * 60 * 60);

/// Spawns the background update checker.
///
/// 1. Cleans up stale update files from previous sessions.
/// 2. Waits 3 seconds at startup, then checks for a new release.
/// 3. If one exists, downloads it silently in the background and notifies the tray.
/// 4. Re-checks periodically every [`RECHECK_INTERVAL`].
pub fn spawn_update_checker(set: &mut JoinSet<()>, tray: Arc<dyn TrayNotifier>) {
    set.spawn(async move {
        // Clean up stale update files from previous sessions.
        let dir = std::env::temp_dir().join("gemacast-update");
        gemacast_core::updater::cleanup_stale_updates(&dir);

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

        loop {
            check_and_download(current_version, key, &tray).await;

            // Wait before the next check. If the download succeeded the tray
            // already has the "Install Update" item, so subsequent checks are
            // no-ops (the version will still be newer until the user installs).
            tokio::time::sleep(RECHECK_INTERVAL).await;
        }
    });
}

/// Run a single check-and-download cycle.
async fn check_and_download(
    current_version: &str,
    platform_key: &str,
    tray: &Arc<dyn TrayNotifier>,
) {
    let info = match gemacast_core::updater::check_for_update(current_version, platform_key).await {
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
    if let Err(e) = std::fs::create_dir_all(&dir) {
        tracing::warn!("Failed to create update directory: {}", e);
        tray.notify_update_failed(format!("Failed to create update directory: {e}"));
        return;
    }
    let file_path = dir.join(filename);

    match gemacast_core::updater::download_update(
        &info.download_url,
        &file_path,
        None,
        info.sha256.as_deref(),
    )
    .await
    {
        Ok(()) => {
            tray.notify_update_ready(info.version, file_path);
        }
        Err(e) => {
            tracing::warn!("Update download failed: {}", e);
            tray.notify_update_failed(e);
        }
    }
}
