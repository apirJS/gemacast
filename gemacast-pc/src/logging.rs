//! Tracing/logging initialization for the PC sender.
//!
//! `gemacast-pc` is a GUI (tray) binary compiled with
//! `#![windows_subsystem = "windows"]`, which detaches it from any console.
//! That means a plain `tracing_subscriber::fmt` writing to stderr has nowhere
//! to go when the app is launched normally — the previous behavior, and why
//! `tracing::*` output was invisible even with `RUST_LOG=info`.
//!
//! This module fixes both halves:
//!   1. On Windows, attach to the parent process's console (if the app was
//!      launched from a terminal) so stderr is actually displayed.
//!   2. Install an `EnvFilter`-backed subscriber so `RUST_LOG` is honored,
//!      defaulting to `info` when unset.

/// Attach to the parent console on Windows so a GUI-subsystem binary can print
/// to the terminal it was launched from. No-op if there is no parent console
/// (e.g. launched from Explorer or autostart) — logging then simply has no
/// visible sink, which is fine for a background tray app.
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

/// Initialize global tracing. Idempotent-safe via `try_init` (never panics if a
/// subscriber is already set, e.g. in tests). Call once, as early as possible.
pub fn init() {
    attach_parent_console();

    use tracing_subscriber::EnvFilter;
    // RUST_LOG if present, else default to `info`. `gemacast_core` and this crate
    // both emit under their own targets, so `RUST_LOG=gemacast_core=debug` works.
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        // A GUI binary's attached console is a real terminal, but redirected
        // output (file/pipe) is common too; keep ANSI on — Windows Terminal and
        // modern consoles render it, and the escapes are harmless in log files.
        .with_target(true)
        .try_init();
}
