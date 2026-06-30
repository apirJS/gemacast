//! Persistent user configuration stored as JSON.
//!
//! Config path: `<config_dir>/gemacast/config.json`
//!
//! - **Windows**: `C:\Users\<user>\AppData\Roaming\gemacast\config.json`
//! - **Linux**:   `~/.config/gemacast/config.json`
//! - **macOS**:   `~/Library/Application Support/gemacast/config.json`

use serde::{Deserialize, Serialize};
use std::io;
use std::path::PathBuf;

/// Persistent user preferences.
///
/// Unknown keys in the JSON file are silently ignored (forward-compatible).
/// Missing keys use `#[serde(default)]` so old config files from before a
/// field was added just get the default value.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserConfig {
    /// Launch the application automatically when the user logs in.
    #[serde(default = "default_true")]
    pub launch_on_startup: bool,

    /// Whether the one-time "running in background" welcome dialog has been shown.
    #[serde(default)]
    pub welcome_dialog_shown: bool,
}

fn default_true() -> bool {
    true
}

impl Default for UserConfig {
    fn default() -> Self {
        Self {
            launch_on_startup: true,
            welcome_dialog_shown: false,
        }
    }
}

/// Returns the path to the config file.
///
/// Falls back to `./gemacast/config.json` if the OS config directory
/// cannot be determined (shouldn't happen on any supported platform).
pub fn config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("gemacast")
        .join("config.json")
}

/// Load config from disk, falling back to defaults on any error.
pub fn load_config() -> UserConfig {
    let path = config_path();
    match std::fs::read_to_string(&path) {
        Ok(contents) => serde_json::from_str(&contents).unwrap_or_default(),
        Err(_) => UserConfig::default(),
    }
}

/// Save config to disk atomically (write to temp file, then rename).
///
/// Creates parent directories if they don't exist.
pub fn save_config(config: &UserConfig) -> io::Result<()> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let json = serde_json::to_string_pretty(config)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

    // Atomic write: write to a sibling temp file, then rename.
    let tmp_path = path.with_extension("json.tmp");
    std::fs::write(&tmp_path, json.as_bytes())?;
    std::fs::rename(&tmp_path, &path)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_should_have_launch_on_startup_true() {
        let cfg = UserConfig::default();
        assert!(cfg.launch_on_startup);
        assert!(!cfg.welcome_dialog_shown);
    }

    #[test]
    fn should_deserialize_empty_json_to_defaults() {
        let cfg: UserConfig = serde_json::from_str("{}").unwrap();
        assert!(cfg.launch_on_startup);
        assert!(!cfg.welcome_dialog_shown);
    }

    #[test]
    fn should_deserialize_partial_json_keeping_defaults_for_missing_fields() {
        let cfg: UserConfig = serde_json::from_str(r#"{"welcome_dialog_shown": true}"#).unwrap();
        assert!(cfg.launch_on_startup); // default
        assert!(cfg.welcome_dialog_shown); // overridden
    }

    #[test]
    fn should_round_trip_through_json() {
        let original = UserConfig {
            launch_on_startup: false,
            welcome_dialog_shown: true,
        };
        let json = serde_json::to_string(&original).unwrap();
        let restored: UserConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(original.launch_on_startup, restored.launch_on_startup);
        assert_eq!(original.welcome_dialog_shown, restored.welcome_dialog_shown);
    }

    #[test]
    fn should_ignore_unknown_fields() {
        let cfg: UserConfig =
            serde_json::from_str(r#"{"launch_on_startup": false, "future_field": 42}"#).unwrap();
        assert!(!cfg.launch_on_startup);
    }
}
