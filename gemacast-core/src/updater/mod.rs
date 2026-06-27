use futures::StreamExt;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Write;
use std::path::Path;
use tokio::sync::mpsc;

/// A single platform entry in `updater.json`.
#[derive(Debug, Clone, Deserialize)]
pub struct PlatformEntry {
    pub url: String,
    #[allow(dead_code)]
    pub signature: String,
}

/// The top-level structure of the `updater.json` file produced by the release
/// pipeline.
#[derive(Debug, Clone, Deserialize)]
pub struct UpdateManifest {
    pub version: String,
    #[allow(dead_code)]
    pub pub_date: String,
    pub platforms: HashMap<String, PlatformEntry>,
}

/// Information about an available update.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    pub version: String,
    pub download_url: String,
}

/// The URL that always resolves to the latest release's updater manifest.
pub const UPDATER_URL: &str =
    "https://github.com/apirJS/gemacast/releases/latest/download/updater.json";

/// Check whether an update is available for the given platform key by fetching the remote manifest.
///
/// Returns `Ok(Some(info))` when a newer version exists, `Ok(None)` when the
/// app is already up-to-date, or `Err` on network / parse failures.
pub async fn check_for_update(
    current_version: &str,
    platform_key: &str,
) -> Result<Option<UpdateInfo>, String> {
    let current = semver::Version::parse(current_version)
        .map_err(|e| format!("Bad current version '{current_version}': {e}"))?;

    tracing::info!("Checking for updates (current: v{current})...");

    let body = reqwest::get(UPDATER_URL)
        .await
        .map_err(|e| format!("Failed to fetch updater manifest: {e}"))?
        .error_for_status()
        .map_err(|e| {
            format!(
                "Updater manifest request failed (HTTP {}): {e}",
                e.status().map_or("unknown".to_string(), |s| s.to_string())
            )
        })?
        .text()
        .await
        .map_err(|e| format!("Failed to read updater manifest body: {e}"))?;

    let manifest: UpdateManifest = serde_json::from_str(&body)
        .map_err(|e| format!("Failed to parse updater manifest: {e}"))?;

    let remote = semver::Version::parse(&manifest.version)
        .map_err(|e| format!("Bad remote version '{}': {e}", manifest.version))?;

    if remote <= current {
        tracing::info!("App is up to date (v{current})");
        return Ok(None);
    }

    let entry = manifest
        .platforms
        .get(platform_key)
        .ok_or_else(|| format!("No entry for platform '{platform_key}' in updater manifest"))?;

    tracing::info!("Update available: v{}", manifest.version);

    Ok(Some(UpdateInfo {
        version: manifest.version,
        download_url: entry.url.clone(),
    }))
}

/// Download an update to the given file path, optionally reporting progress (0–100)
/// via the provided `progress_tx` channel.
pub async fn download_update(
    url: &str,
    file_path: &Path,
    progress_tx: Option<mpsc::Sender<u8>>,
) -> Result<(), String> {
    let response = reqwest::get(url)
        .await
        .map_err(|e| format!("Download request failed: {e}"))?
        .error_for_status()
        .map_err(|e| {
            format!(
                "Download request failed (HTTP {}): {e}",
                e.status().map_or("unknown".to_string(), |s| s.to_string())
            )
        })?;

    let total_size = response.content_length().unwrap_or(0);

    let mut file = std::fs::File::create(file_path)
        .map_err(|e| format!("Failed to create download file: {e}"))?;

    let mut downloaded: u64 = 0;
    let mut last_percent: u8 = 0;
    let mut stream = response.bytes_stream();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("Download stream error: {e}"))?;
        file.write_all(&chunk)
            .map_err(|e| format!("Failed to write to file: {e}"))?;
        downloaded += chunk.len() as u64;

        if total_size > 0 {
            let percent = ((downloaded as f64 / total_size as f64) * 100.0).min(100.0) as u8;
            if percent != last_percent {
                last_percent = percent;
                if let Some(tx) = &progress_tx {
                    let _ = tx.try_send(percent);
                }
            }
        }
    }

    // Flush all data to disk before we hand the path to the installer.
    file.sync_all()
        .map_err(|e| format!("Failed to sync downloaded file to disk: {e}"))?;

    // Ensure we always report 100%.
    if let Some(tx) = &progress_tx {
        let _ = tx.try_send(100);
    }

    tracing::info!(
        "Downloaded update to {} ({} bytes)",
        file_path.display(),
        downloaded
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_updater_json() {
        let json = r#"{
            "version": "0.2.0",
            "pub_date": "2026-06-26T00:00:00Z",
            "platforms": {
                "windows-x86_64": {
                    "url": "https://example.com/installer.msi",
                    "signature": "https://example.com/installer.msi.sig"
                }
            }
        }"#;

        let manifest: UpdateManifest = serde_json::from_str(json).unwrap();
        assert_eq!(manifest.version, "0.2.0");
        assert!(manifest.platforms.contains_key("windows-x86_64"));
    }
}
