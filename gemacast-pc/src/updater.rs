//! Auto-update platform helpers and installer execution.
//!
//! The checking and downloading logic is handled by `gemacast_core::updater`.

use std::path::Path;

/// Returns the `updater.json` platform key for the current build target.
pub fn platform_key() -> Option<&'static str> {
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    return Some("windows-x86_64");

    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    return Some("darwin-x86_64");

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    return Some("darwin-aarch64");

    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    return Some("linux-x86_64");

    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    return Some("linux-aarch64");

    #[cfg(not(any(
        all(target_os = "windows", target_arch = "x86_64"),
        all(target_os = "macos", target_arch = "x86_64"),
        all(target_os = "macos", target_arch = "aarch64"),
        all(target_os = "linux", target_arch = "x86_64"),
        all(target_os = "linux", target_arch = "aarch64")
    )))]
    return None;
}

/// Launch the downloaded installer.
///
/// On Windows this opens the `.msi` with the default handler.
/// On macOS it opens the `.dmg`.
/// On Linux it replaces the current AppImage binary.
pub fn install_update(installer_path: &Path) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        use std::path::PathBuf;
        // For Linux AppImage: replace the original binary and restart.
        let target_path = if let Ok(appimage_path) = std::env::var("APPIMAGE") {
            PathBuf::from(appimage_path)
        } else {
            std::env::current_exe().unwrap_or_else(|_| PathBuf::from("gemacast-pc"))
        };

        // Make the downloaded AppImage executable.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ =
                std::fs::set_permissions(installer_path, std::fs::Permissions::from_mode(0o755));
        }

        std::fs::copy(installer_path, &target_path).map_err(|e| {
            format!(
                "Failed to replace binary at {}: {}",
                target_path.display(),
                e
            )
        })?;

        // Restart the application.
        let _ = std::process::Command::new(target_path).spawn();
        std::process::exit(0);
    }

    #[cfg(not(target_os = "linux"))]
    {
        // Windows (.msi) / macOS (.dmg) — open with the system handler.
        open::that(installer_path).map_err(|e| format!("Failed to open installer: {e}"))?;
        Ok(())
    }
}

/// Delete the downloaded installer file (used when the user cancels).
pub fn cleanup_update(installer_path: &Path) {
    match std::fs::remove_file(installer_path) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            tracing::warn!(
                "Failed to clean up downloaded installer at {}: {}",
                installer_path.display(),
                e
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_key_is_not_empty() {
        if let Some(key) = platform_key() {
            assert!(!key.is_empty());
        }
    }
}
