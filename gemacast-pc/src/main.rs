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

/// Wait for any termination signal (Ctrl+C, stdin "quit", or OS-specific signals).
async fn wait_for_termination() {
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::mpsc::channel::<()>(1);

    // Ctrl+C
    let tx = shutdown_tx.clone();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        let _ = tx.send(()).await;
    });

    #[cfg(windows)]
    {
        let tx = shutdown_tx.clone();
        tokio::spawn(async move {
            if let Ok(mut signal) = tokio::signal::windows::ctrl_close() {
                signal.recv().await;
                let _ = tx.send(()).await;
            }
        });

        let tx = shutdown_tx.clone();
        tokio::spawn(async move {
            if let Ok(mut signal) = tokio::signal::windows::ctrl_break() {
                signal.recv().await;
                let _ = tx.send(()).await;
            }
        });
    }

    #[cfg(unix)]
    {
        let tx = shutdown_tx.clone();
        tokio::spawn(async move {
            if let Ok(mut sigterm) =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            {
                sigterm.recv().await;
                let _ = tx.send(()).await;
            }
        });
    }

    // Stdin "quit" command
    let tx = shutdown_tx.clone();
    tokio::task::spawn_blocking(move || {
        let stdin = std::io::stdin();
        let mut line = String::new();
        while let Ok(bytes) = stdin.read_line(&mut line) {
            if bytes == 0 {
                break;
            }
            if line.trim().eq_ignore_ascii_case("quit") {
                let _ = tx.blocking_send(());
                break;
            }
            line.clear();
        }
    });

    let _ = shutdown_rx.recv().await;
}

fn main() {
    let _ = tracing_subscriber::fmt::try_init();

    std::thread::spawn(|| {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("Failed to build Tokio runtime for termination listener");

        rt.block_on(async {
            wait_for_termination().await;
            eprintln!("Termination signal received. Exiting gracefully...");
            std::process::exit(0);
        });
    });

    app::run();
}
