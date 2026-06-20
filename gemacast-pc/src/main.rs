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
mod background;
mod events;
mod state;
pub mod tasks;
pub mod traits;
mod tray;

#[cfg(test)]
pub mod testing;

fn main() {
    let _ = tracing_subscriber::fmt::try_init();

    // Enforce single instance via file lock.
    // If another gemacast-pc process already holds the lock, show a
    // user-friendly dialog and exit immediately — before any ports are bound.
    let lock_dir = std::env::temp_dir().join("gemacast");
    let _ = std::fs::create_dir_all(&lock_dir);
    let lock_path = lock_dir.join("gemacast-pc.lock");
    let lock_file = match std::fs::File::create(&lock_path) {
        Ok(f) => f,
        Err(e) => {
            tracing::error!("Failed to create lock file at {}: {}", lock_path.display(), e);
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

    app::run();
}
