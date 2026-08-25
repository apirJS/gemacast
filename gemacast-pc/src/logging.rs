//! Tracing/logging initialization for the PC streamer.
//!
//! stderr only, and nothing is persisted. Diagnostics for a shipped build come
//! from [`crate::crash_log`], which is the one artifact worth keeping; the
//! `tracing` macros themselves are compiled out of release builds by the
//! `release_max_level_off` feature on the `tracing` dependency, so in a shipped
//! binary this subscriber has nothing to receive. It exists for `cargo run`.
//!
//! `gemacast-pc` is a GUI (tray) binary compiled with
//! `#![windows_subsystem = "windows"]`, which detaches it from any console, so on
//! Windows we first attach to the parent process's console — otherwise a developer
//! running the binary from a terminal sees nothing even with `RUST_LOG=debug`.

/// Attach to the parent console on Windows so a GUI-subsystem binary can print
/// to the terminal it was launched from. No-op if there is no parent console
/// (e.g. launched from Explorer or autostart), where stderr has no visible sink
/// at all — that path is what the crash log covers.
#[cfg(target_os = "windows")]
fn attach_parent_console() {
    use windows_sys::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};
    // SAFETY: AttachConsole is a simple FFI call with no memory contract; it
    // returns 0 on failure (no parent console), which we ignore.
    unsafe {
        let _ = AttachConsole(ATTACH_PARENT_PROCESS);
    }
}

#[cfg(not(target_os = "windows"))]
fn attach_parent_console() {}

/// Initialize global tracing.
///
/// Idempotent-safe via `try_init` (never panics if a subscriber is already set,
/// e.g. in tests). Call once, as early as possible.
pub fn init() {
    attach_parent_console();

    // RUST_LOG if present, else default to `info`. `gemacast_core` and this crate
    // both emit under their own targets, so `RUST_LOG=gemacast_core=debug` works.
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(true)
        .try_init();
}
