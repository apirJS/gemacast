use futures::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::Write;
use std::path::Path;
use tokio::sync::mpsc;

/// Maximum number of retry attempts for network operations.
const MAX_RETRIES: u32 = 3;

/// Initial backoff duration in milliseconds (doubles each retry).
const INITIAL_BACKOFF_MS: u64 = 1_000;

/// Length of a SHA-256 digest written as hex.
const SHA256_HEX_LEN: usize = 64;

/// A single platform entry in `updater.json`.
#[derive(Debug, Clone, Deserialize)]
pub struct PlatformEntry {
    pub url: String,
    /// URL of the detached GPG signature for the artifact, produced by
    /// `.github/scripts/sign-release-assets.sh`.
    ///
    /// **Not verified in-app**, deliberately — there is no pinned public key in
    /// the binary to verify it against. It is published so a user can check a
    /// download out of band. In-app integrity rests entirely on [`Self::sha256`]
    /// plus the fact that the manifest itself is fetched over HTTPS, so do not
    /// read the presence of this field as an integrity guarantee.
    pub signature: String,
    /// SHA-256 hex digest of the downloadable artifact.
    ///
    /// `Option` only so that a legacy or malformed manifest fails with a clear
    /// message from [`Self::verified_sha256`] instead of an opaque serde error.
    /// The digest is **required**: an entry without a well-formed one is refused
    /// rather than installed unverified.
    #[serde(default)]
    pub sha256: Option<String>,
}

impl PlatformEntry {
    /// Return the artifact's digest, rejecting any entry that lacks a usable one.
    ///
    /// This digest is the only integrity check the app performs on a downloaded
    /// update, so a missing, empty, or malformed value must fail the update
    /// instead of skipping verification. Both degenerate shapes are real pipeline
    /// outcomes rather than hand-edited manifests: entries predating the field
    /// omit it entirely, and `generate-updater.sh` used to emit `sha256: ""` when
    /// it could not fetch an artifact.
    fn verified_sha256(&self, platform_key: &str) -> Result<String, String> {
        let digest = self.sha256.as_deref().unwrap_or_default().trim();

        if digest.is_empty() {
            return Err(format!(
                "Updater manifest entry for '{platform_key}' carries no sha256 digest; \
                 refusing to install an unverified update"
            ));
        }

        if digest.len() != SHA256_HEX_LEN || !digest.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(format!(
                "Updater manifest entry for '{platform_key}' has a malformed sha256 digest \
                 ({} chars, expected {SHA256_HEX_LEN} hex digits); refusing to install",
                digest.len()
            ));
        }

        Ok(digest.to_ascii_lowercase())
    }
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
    pub sha256: String,
}

/// The URL that always resolves to the latest release's updater manifest.
pub const UPDATER_URL: &str =
    "https://github.com/apirJS/gemacast/releases/latest/download/updater.json";

/// Check whether an update is available for the given platform key by fetching the remote manifest.
///
/// Returns `Ok(Some(info))` when a newer version exists, `Ok(None)` when the
/// app is already up-to-date, or `Err` on network / parse failures.
///
/// Retries transient network failures up to [`MAX_RETRIES`] times with
/// exponential backoff.
pub async fn check_for_update(
    current_version: &str,
    platform_key: &str,
) -> Result<Option<UpdateInfo>, String> {
    let current = semver::Version::parse(current_version)
        .map_err(|e| format!("Bad current version '{current_version}': {e}"))?;

    tracing::info!("Checking for updates (current: v{current})...");

    let body = retry_async(MAX_RETRIES, INITIAL_BACKOFF_MS, || async {
        let resp = reqwest::get(UPDATER_URL)
            .await
            .map_err(|e| format!("Failed to fetch updater manifest: {e}"))?
            .error_for_status()
            .map_err(|e| {
                format!(
                    "Updater manifest request failed (HTTP {}): {e}",
                    e.status().map_or("unknown".to_string(), |s| s.to_string())
                )
            })?;
        resp.text()
            .await
            .map_err(|e| format!("Failed to read updater manifest body: {e}"))
    })
    .await?;

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
        sha256: entry.verified_sha256(platform_key)?,
    }))
}

/// Download an update to the given file path, optionally reporting progress (0-100)
/// via the provided `progress_tx` channel.
///
/// The download is always verified against `expected_sha256`, and a mismatch both
/// errors and deletes the file. The digest is not optional: taking `&str` rather
/// than `Option<&str>` is what makes "install without verifying" impossible to
/// express at this call site, including from the Tauri command surface, where the
/// digest would otherwise round-trip through JavaScript as a nullable value.
///
/// Retries the initial HTTP request up to [`MAX_RETRIES`] times with exponential
/// backoff. Once the stream starts, individual chunk errors are not retried (the
/// download must be restarted).
pub async fn download_update(
    url: &str,
    file_path: &Path,
    progress_tx: Option<mpsc::Sender<u8>>,
    expected_sha256: &str,
) -> Result<(), String> {
    let response = retry_async(MAX_RETRIES, INITIAL_BACKOFF_MS, || {
        let url = url.to_string();
        async move {
            reqwest::get(&url)
                .await
                .map_err(|e| format!("Download request failed: {e}"))?
                .error_for_status()
                .map_err(|e| {
                    format!(
                        "Download request failed (HTTP {}): {e}",
                        e.status().map_or("unknown".to_string(), |s| s.to_string())
                    )
                })
        }
    })
    .await?;

    let total_size = response.content_length().unwrap_or(0);

    let mut file = std::fs::File::create(file_path)
        .map_err(|e| format!("Failed to create download file: {e}"))?;

    let mut hasher = Sha256::new();
    let mut downloaded: u64 = 0;
    let mut last_percent: u8 = 0;
    let mut stream = response.bytes_stream();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("Download stream error: {e}"))?;
        file.write_all(&chunk)
            .map_err(|e| format!("Failed to write to file: {e}"))?;
        hasher.update(&chunk);
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

    // Verify the SHA-256 digest before anyone is handed this path to install.
    let actual = hex::encode(hasher.finalize());
    if !actual.eq_ignore_ascii_case(expected_sha256) {
        // Remove the file so a failed verification cannot leave an installable
        // artifact behind for a later code path to pick up.
        let _ = std::fs::remove_file(file_path);
        return Err(format!(
            "SHA-256 mismatch: expected {expected_sha256}, got {actual}. \
             The download may be corrupted or tampered with."
        ));
    }
    tracing::info!("SHA-256 checksum verified for {}", file_path.display());

    tracing::info!(
        "Downloaded update to {} ({} bytes)",
        file_path.display(),
        downloaded
    );

    Ok(())
}

/// Delete stale update files in the given directory.
///
/// Call this on startup to reclaim space from previously downloaded installers
/// that were never installed (or have already been applied).
pub fn cleanup_stale_updates(dir: &Path) {
    match std::fs::read_dir(dir) {
        Ok(entries) => {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() {
                    match std::fs::remove_file(&path) {
                        Ok(()) => {
                            tracing::debug!("Cleaned up stale update file: {}", path.display())
                        }
                        Err(e) => {
                            tracing::warn!(
                                "Failed to clean up stale update file {}: {}",
                                path.display(),
                                e
                            )
                        }
                    }
                }
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            tracing::warn!("Failed to read update directory {}: {}", dir.display(), e);
        }
    }
}

/// Retry an async operation with exponential backoff.
async fn retry_async<F, Fut, T>(
    max_retries: u32,
    initial_backoff_ms: u64,
    op: F,
) -> Result<T, String>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<T, String>>,
{
    let mut backoff_ms = initial_backoff_ms;
    let mut last_err = String::new();

    for attempt in 0..=max_retries {
        match op().await {
            Ok(val) => return Ok(val),
            Err(e) => {
                last_err = e;
                if attempt < max_retries {
                    tracing::warn!(
                        "Attempt {}/{} failed: {}. Retrying in {}ms...",
                        attempt + 1,
                        max_retries + 1,
                        last_err,
                        backoff_ms
                    );
                    tokio::time::sleep(tokio::time::Duration::from_millis(backoff_ms)).await;
                    backoff_ms *= 2;
                }
            }
        }
    }

    Err(last_err)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a manifest with the given raw `sha256` JSON fragment for the one
    /// platform entry. Pass an empty string to omit the field entirely.
    fn manifest_with_sha256(fragment: &str) -> UpdateManifest {
        let json = format!(
            r#"{{
                "version": "0.3.0",
                "pub_date": "2026-06-28T00:00:00Z",
                "platforms": {{
                    "windows-x86_64": {{
                        "url": "https://example.com/installer.msi",
                        "signature": "https://example.com/installer.msi.sig"
                        {fragment}
                    }}
                }}
            }}"#
        );
        serde_json::from_str(&json).unwrap()
    }

    const VALID_DIGEST: &str = "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08";

    #[test]
    fn parse_updater_json() {
        let manifest = manifest_with_sha256(&format!(r#", "sha256": "{VALID_DIGEST}""#));
        assert_eq!(manifest.version, "0.3.0");
        assert!(manifest.platforms.contains_key("windows-x86_64"));
        assert_eq!(
            manifest.platforms["windows-x86_64"]
                .verified_sha256("windows-x86_64")
                .unwrap(),
            VALID_DIGEST
        );
    }

    #[test]
    fn a_digest_is_normalized_to_lowercase_so_it_compares_consistently() {
        let manifest =
            manifest_with_sha256(&format!(r#", "sha256": "{}""#, VALID_DIGEST.to_uppercase()));
        assert_eq!(
            manifest.platforms["windows-x86_64"]
                .verified_sha256("windows-x86_64")
                .unwrap(),
            VALID_DIGEST
        );
    }

    // The three ways a manifest can fail to carry a usable digest. Each must be
    // refused: the digest is the only integrity check on a downloaded update, so
    // skipping verification when it is absent would let a manifest disable the
    // very check that is meant to constrain it.

    #[test]
    fn an_entry_without_a_digest_is_refused_rather_than_installed_unverified() {
        // Legacy manifests predating the sha256 field deserialize to None.
        let manifest = manifest_with_sha256("");
        let err = manifest.platforms["windows-x86_64"]
            .verified_sha256("windows-x86_64")
            .unwrap_err();
        assert!(err.contains("no sha256 digest"), "unexpected error: {err}");
    }

    #[test]
    fn an_empty_digest_is_refused_rather_than_treated_as_a_mismatch() {
        // generate-updater.sh used to emit "" when it could not fetch an artifact.
        // Left to reach download_update, this produced a confusing "SHA-256
        // mismatch: expected , got <hash>" instead of naming the real problem.
        let manifest = manifest_with_sha256(r#", "sha256": "  ""#);
        let err = manifest.platforms["windows-x86_64"]
            .verified_sha256("windows-x86_64")
            .unwrap_err();
        assert!(err.contains("no sha256 digest"), "unexpected error: {err}");
    }

    #[test]
    fn a_malformed_digest_is_refused_before_any_download_starts() {
        for bad in [
            "abc123def456",
            "zz86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a0",
        ] {
            let manifest = manifest_with_sha256(&format!(r#", "sha256": "{bad}""#));
            let err = manifest.platforms["windows-x86_64"]
                .verified_sha256("windows-x86_64")
                .unwrap_err();
            assert!(
                err.contains("malformed sha256 digest"),
                "expected {bad:?} to be rejected as malformed, got: {err}"
            );
        }
    }

    #[tokio::test]
    async fn retry_succeeds_on_first_try() {
        let result = retry_async(3, 10, || async { Ok::<_, String>(42) }).await;
        assert_eq!(result.unwrap(), 42);
    }

    #[tokio::test]
    async fn retry_returns_last_error_after_exhaustion() {
        let counter = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let c = counter.clone();
        let result = retry_async(2, 10, move || {
            let c = c.clone();
            async move {
                c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Err::<(), _>("always fails".to_string())
            }
        })
        .await;
        assert!(result.is_err());
        // 1 initial + 2 retries = 3 total attempts
        assert_eq!(counter.load(std::sync::atomic::Ordering::SeqCst), 3);
    }
}
