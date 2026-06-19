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
    app::run();
}
