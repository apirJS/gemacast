use std::sync::Arc;

use crate::traits::FrontendNotifier;

/// Runs the IPC service command listener loop.
///
/// This is an `async fn` — the caller is responsible for spawning it
/// on the appropriate runtime (e.g. `tauri::async_runtime::spawn`).
pub async fn run_service_command_listener(
    notifier: Arc<dyn FrontendNotifier>,
    cache_dir: Option<std::path::PathBuf>,
) {
    let addr = std::net::SocketAddrV4::new(std::net::Ipv4Addr::new(127, 0, 0, 1), 0);
    let Ok(socket) = tokio::net::UdpSocket::bind(addr).await else {
        return;
    };

    if let Ok(local_addr) = socket.local_addr() {
        if let Some(dir) = &cache_dir {
            let _ = std::fs::create_dir_all(dir);
            let _ = std::fs::write(dir.join(".ipc_port"), local_addr.port().to_string());
        }
    }

    let mut buf = vec![0u8; 1024];
    while let Ok((len, _)) = socket.recv_from(&mut buf).await {
        let Ok(command) = std::str::from_utf8(&buf[..len]) else {
            continue;
        };

        // The frontend handles all connect/disconnect logic via the
        // `service-command` event listener. The Rust side should NOT
        // duplicate this logic, as doing so causes race conditions
        // (e.g., Rust kills the session while the frontend also tries
        // to tear it down, or Rust sets is_playing while the frontend
        // spawns a whole new session).
        notifier.emit_service_command(command.to_string());
    }
}
