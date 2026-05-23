mod app;
pub mod domains;
mod events;
mod network;
mod state;
mod tray;

async fn wait_for_termination() {
    let (daemon_shutdown_tx, mut daemon_shutdown_rx) = tokio::sync::mpsc::channel::<()>(1);

    let ctrl_c_shutdown_tx = daemon_shutdown_tx.clone();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        let _ = ctrl_c_shutdown_tx.send(()).await;
    });

    #[cfg(windows)]
    {
        let ctrl_close_shutdown_tx = daemon_shutdown_tx.clone();
        tokio::spawn(async move {
            if let Ok(mut ctrl_close) = tokio::signal::windows::ctrl_close() {
                ctrl_close.recv().await;
                let _ = ctrl_close_shutdown_tx.send(()).await;
            }
        });

        let ctrl_break_shutdown_tx = daemon_shutdown_tx.clone();
        tokio::spawn(async move {
            if let Ok(mut ctrl_break) = tokio::signal::windows::ctrl_break() {
                ctrl_break.recv().await;
                let _ = ctrl_break_shutdown_tx.send(()).await;
            }
        });
    }

    #[cfg(unix)]
    {
        let sigterm_shutdown_tx = daemon_shutdown_tx.clone();
        tokio::spawn(async move {
            if let Ok(mut sigterm) =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            {
                sigterm.recv().await;
                let _ = sigterm_shutdown_tx.send(()).await;
            }
        });
    }

    let stdin_shutdown_tx = daemon_shutdown_tx.clone();
    tokio::task::spawn_blocking(move || {
        let stdin = std::io::stdin();
        let mut line = String::new();
        while let Ok(bytes) = stdin.read_line(&mut line) {
            if bytes == 0 {
                break;
            }
            if line.trim().eq_ignore_ascii_case("quit") {
                let _ = stdin_shutdown_tx.blocking_send(());
                break;
            }
            line.clear();
        }
    });

    let _ = daemon_shutdown_rx.recv().await;
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
