mod app;
pub mod domains;
mod events;
mod network;
mod state;
mod tray;

async fn wait_for_termination() {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<()>(1);
    
    let tx1 = tx.clone();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        let _ = tx1.send(()).await;
    });

    #[cfg(windows)]
    {
        let tx2 = tx.clone();
        tokio::spawn(async move {
            if let Ok(mut ctrl_close) = tokio::signal::windows::ctrl_close() {
                ctrl_close.recv().await;
                let _ = tx2.send(()).await;
            }
        });

        let tx3 = tx.clone();
        tokio::spawn(async move {
            if let Ok(mut ctrl_break) = tokio::signal::windows::ctrl_break() {
                ctrl_break.recv().await;
                let _ = tx3.send(()).await;
            }
        });
    }

    #[cfg(unix)]
    {
        let tx4 = tx.clone();
        tokio::spawn(async move {
            if let Ok(mut sigterm) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
                sigterm.recv().await;
                let _ = tx4.send(()).await;
            }
        });
    }

    let tx5 = tx.clone();
    tokio::task::spawn_blocking(move || {
        let stdin = std::io::stdin();
        let mut line = String::new();
        while let Ok(bytes) = stdin.read_line(&mut line) {
            if bytes == 0 { break; }
            if line.trim().eq_ignore_ascii_case("quit") {
                let _ = tx5.blocking_send(());
                break;
            }
            line.clear();
        }
    });

    let _ = rx.recv().await;
}

fn main() {
    std::thread::spawn(|| {
        if let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() {
            rt.block_on(async {
                wait_for_termination().await;
                eprintln!("Termination signal received. Exiting gracefully...");
                std::process::exit(0);
            });
        }
    });

    app::run();
}
