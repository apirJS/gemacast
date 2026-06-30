#![windows_subsystem = "windows"]

//! GemaCast PC Sender — streams desktop audio to mobile devices.
//!
//! This binary runs as a system tray application. The main thread owns the
//! tray event loop ([`app`]), while a background thread runs all async tasks
//! ([`background`]) for device discovery, audio streaming, and control.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────┐     AppCommand      ┌───────────────────┐
//! │  Main Thread (tray/UI)      │ ──────────────────►  │  Background Engine │
//! │  app.rs + tray.rs           │ ◄──────────────────  │  background.rs     │
//! └─────────────────────────────┘     TrayEvent       │  └─► tasks/*        │
//!                                                      └───────────────────┘
//! ```

mod adapters;
mod app;
mod autostart;
mod background;
mod config;
mod crash_log;
mod events;
mod state;
pub mod tasks;
pub mod traits;
mod tray;
mod updater;

#[cfg(test)]
pub mod testing;

fn main() {
    // Install the crash-log panic hook as early as possible so even
    // initialization panics are captured to disk.
    crash_log::install_panic_hook();

    let _ = tracing_subscriber::fmt::try_init();

    // Purge old crash logs (best-effort, never fails).
    crash_log::cleanup_old_crash_logs();

    // Enforce single instance via file lock.
    // If another gemacast-pc process already holds the lock, show a
    // user-friendly dialog and exit immediately — before any ports are bound.
    let lock_dir = std::env::temp_dir().join("gemacast");
    let _ = std::fs::create_dir_all(&lock_dir);
    let lock_path = lock_dir.join("gemacast-pc.lock");
    let lock_file = match std::fs::File::create(&lock_path) {
        Ok(f) => f,
        Err(e) => {
            tracing::error!(
                "Failed to create lock file at {}: {}",
                lock_path.display(),
                e
            );
            app::run();
            return;
        }
    };

    use fs2::FileExt;
    if lock_file.try_lock_exclusive().is_err() {
        rfd::MessageDialog::new()
            .set_title("GemaCast")
            .set_description("GemaCast is already running! Check your system tray.")
            .set_level(rfd::MessageLevel::Info)
            .show();
        return;
    }

    // Keep _lock_guard alive for the entire process lifetime.
    // The lock is automatically released when the process exits.
    let _lock_guard = lock_file;

    // -- Load user config and sync platform state -------------------------
    let mut user_config = config::load_config();

    // Sync autostart with config (self-healing: if user manually deleted
    // the registry entry / .desktop file, we re-create it).
    if let Err(e) = autostart::set_autostart(user_config.launch_on_startup) {
        tracing::warn!("Failed to sync autostart state: {}", e);
    }

    // Show the one-time welcome dialog for first-time users.
    if !user_config.welcome_dialog_shown {
        rfd::MessageDialog::new()
            .set_title("Gemacast")
            .set_description(
                "Gemacast is running in the background\n\
                 Click the system tray icon to access :)",
            )
            .set_level(rfd::MessageLevel::Info)
            .show();

        user_config.welcome_dialog_shown = true;
        if let Err(e) = config::save_config(&user_config) {
            tracing::warn!("Failed to persist config after welcome dialog: {}", e);
        }
    }

    app::run();
}
