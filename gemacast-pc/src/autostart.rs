//! Launch-on-startup management for all desktop platforms.
//!
//! | Platform | Mechanism                                          |
//! |----------|----------------------------------------------------|
//! | Windows  | `HKCU\SOFTWARE\Microsoft\Windows\CurrentVersion\Run` |
//! | Linux    | `~/.config/autostart/gemacast-pc.desktop`           |
//! | macOS    | `~/Library/LaunchAgents/com.apir.gemacast.plist`    |

use std::io;

/// Enable or disable launch-on-startup for the current platform.
pub fn set_autostart(enabled: bool) -> io::Result<()> {
    platform::set_autostart(enabled)
}

/// Check whether launch-on-startup is currently enabled.
pub fn is_autostart_enabled() -> bool {
    platform::is_autostart_enabled()
}

// ---------------------------------------------------------------------------
// Windows
// ---------------------------------------------------------------------------
#[cfg(target_os = "windows")]
mod platform {
    use std::io;

    const RUN_KEY: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Run";
    const VALUE_NAME: &str = "Gemacast";

    pub fn set_autostart(enabled: bool) -> io::Result<()> {
        use winreg::RegKey;
        use winreg::enums::*;

        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let (key, _) = hkcu.create_subkey(RUN_KEY).map_err(io::Error::other)?;

        if enabled {
            let exe = std::env::current_exe()?;
            // Quote the path in case it contains spaces.
            let value = format!("\"{}\"", exe.display());
            key.set_value(VALUE_NAME, &value)
                .map_err(io::Error::other)?;
        } else {
            // Ignore error if value doesn't exist.
            let _ = key.delete_value(VALUE_NAME);
        }
        Ok(())
    }

    pub fn is_autostart_enabled() -> bool {
        use winreg::RegKey;
        use winreg::enums::*;

        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let Ok(key) = hkcu.open_subkey(RUN_KEY) else {
            return false;
        };
        key.get_value::<String, _>(VALUE_NAME).is_ok()
    }
}

// ---------------------------------------------------------------------------
// Linux
// ---------------------------------------------------------------------------
#[cfg(target_os = "linux")]
mod platform {
    use std::io;
    use std::path::PathBuf;

    fn desktop_file_path() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join("autostart").join("gemacast-pc.desktop"))
    }

    pub fn set_autostart(enabled: bool) -> io::Result<()> {
        let path = desktop_file_path()
            .ok_or_else(|| io::Error::other("cannot determine XDG config dir"))?;

        if enabled {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let exe = std::env::current_exe()?;
            let contents = format!(
                "[Desktop Entry]\n\
                 Type=Application\n\
                 Name=Gemacast\n\
                 Comment=Low-latency real-time audio streaming from PC to Android\n\
                 Exec={}\n\
                 Icon=gemacast-pc\n\
                 Terminal=false\n\
                 StartupNotify=false\n\
                 X-GNOME-Autostart-enabled=true\n",
                exe.display()
            );
            std::fs::write(&path, contents)?;
        } else {
            let _ = std::fs::remove_file(&path);
        }
        Ok(())
    }

    pub fn is_autostart_enabled() -> bool {
        desktop_file_path().is_some_and(|p| p.exists())
    }
}

// ---------------------------------------------------------------------------
// macOS
// ---------------------------------------------------------------------------
#[cfg(target_os = "macos")]
mod platform {
    use std::io;
    use std::path::PathBuf;

    fn plist_path() -> Option<PathBuf> {
        dirs::home_dir().map(|h| {
            h.join("Library")
                .join("LaunchAgents")
                .join("com.apir.gemacast.plist")
        })
    }

    pub fn set_autostart(enabled: bool) -> io::Result<()> {
        let path =
            plist_path().ok_or_else(|| io::Error::other("cannot determine home directory"))?;

        if enabled {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let exe = std::env::current_exe()?;
            let contents = format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.apir.gemacast</string>
    <key>ProgramArguments</key>
    <array>
        <string>{}</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <false/>
</dict>
</plist>"#,
                exe.display()
            );
            std::fs::write(&path, contents)?;
        } else {
            let _ = std::fs::remove_file(&path);
        }
        Ok(())
    }

    pub fn is_autostart_enabled() -> bool {
        plist_path().is_some_and(|p| p.exists())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_autostart_enabled_should_not_panic() {
        // Just verify it doesn't panic — actual state depends on the system.
        let _ = is_autostart_enabled();
    }
}
