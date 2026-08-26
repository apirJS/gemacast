use crate::network::Ports;
use crate::process::{quiet_command, quiet_tokio_command};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use tokio::task::JoinSet;

/// Path to the bundled ADB binary, resolved once per process.
///
/// Memoized because resolving spawns a process (see [`runs`]). Without this,
/// every ADB command would cost two processes, and the watchdog below runs one
/// every 3 seconds for the life of the app.
fn local_adb_path() -> &'static Path {
    static ADB_PATH: OnceLock<PathBuf> = OnceLock::new();
    ADB_PATH.get_or_init(resolve_adb_path).as_path()
}

/// Find a working ADB: next to our own executable first (how every archive and
/// installer ships it), then the deb/rpm install path, then bare `adb` so a dev
/// box falls back to PATH.
fn resolve_adb_path() -> PathBuf {
    let adb_name = if cfg!(target_os = "windows") {
        "adb.exe"
    } else {
        "adb"
    };

    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        let sibling = dir.join(adb_name);
        if runs(&sibling) {
            return sibling;
        }
    }

    #[cfg(target_os = "linux")]
    {
        let packaged = PathBuf::from("/usr/lib/gemacast/adb");
        if runs(&packaged) {
            return packaged;
        }
    }

    PathBuf::from(adb_name)
}

/// Whether `path` exists and can actually be launched.
fn runs(path: &Path) -> bool {
    path.exists() && quiet_command(path).arg("version").output().is_ok()
}

/// A Tokio Command for the bundled ADB. Never pops a console window.
pub fn adb_command() -> tokio::process::Command {
    quiet_tokio_command(local_adb_path())
}

pub fn spawn_adb_port_forwarding_watchdog(
    set: &mut JoinSet<()>,
    tcp_drop_tx: tokio::sync::broadcast::Sender<()>,
) {
    let managed_ports = [
        format!("tcp:{}", Ports::ADB_AUDIO_TCP),
        format!("tcp:{}", Ports::ADB_DISCOVERY_TCP),
        format!("tcp:{}", Ports::CONTROL),
    ];

    set.spawn(async move {
        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(3));
        let mut drop_rx = tcp_drop_tx.subscribe();

        loop {
            tokio::select! {
                _ = drop_rx.recv() => {
                    if let Ok(output) = adb_command().arg("devices").output().await {
                        let stdout = String::from_utf8_lossy(&output.stdout);
                        for serial in stdout.lines().filter_map(connected_serial) {
                            for port in &managed_ports {
                                let _ = adb_command()
                                    .args(["-s", serial, "reverse", "--remove", port])
                                    .output()
                                    .await;
                            }
                        }
                    }
                    break;
                }
                _ = interval.tick() => {
                    if let Ok(output) = adb_command().arg("devices").output().await
                        && output.status.success() {
                        let stdout = String::from_utf8_lossy(&output.stdout);
                        for serial in stdout.lines().filter_map(connected_serial) {
                                if !serial.is_empty()
                                    && let Ok(c) = adb_command()
                                        .args(["-s", serial, "reverse", "--list"])
                                        .output()
                                        .await
                                    {
                                        let check_out = String::from_utf8_lossy(&c.stdout);
                                        if !check_out.contains(&managed_ports[0]) {
                                            let _ = adb_command()
                                                .args(["-s", serial, "reverse", &managed_ports[0], &managed_ports[0]])
                                                .output()
                                                .await;
                                        }
                                        if !check_out.contains(&managed_ports[1]) {
                                            let _ = adb_command()
                                                .args(["-s", serial, "reverse", &managed_ports[1], &managed_ports[1]])
                                                .output()
                                                .await;
                                        }
                                        if !check_out.contains(&managed_ports[2]) {
                                            let _ = adb_command()
                                                .args(["-s", serial, "reverse", &managed_ports[2], &managed_ports[2]])
                                                .output()
                                                .await;
                                        }
                                    }
                        }
                    }
                }
            }
        }
    });
}

fn connected_serial(line: &str) -> Option<&str> {
    let mut fields = line.split_whitespace();
    let serial = fields.next()?;
    (fields.next() == Some("device")).then_some(serial)
}
