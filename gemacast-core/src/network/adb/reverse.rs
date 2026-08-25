use crate::network::Ports;
use tokio::task::JoinSet;

/// Resolve the path to the bundled ADB binary next to our own executable.
fn local_adb_path() -> std::path::PathBuf {
    let adb_name = if cfg!(target_os = "windows") {
        "adb.exe"
    } else {
        "adb"
    };
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        let local = dir.join(adb_name);
        if local.exists() {
            return local;
        }
    }
    std::path::PathBuf::from(adb_name)
}

/// Returns a Tokio Command for the bundled ADB (with CREATE_NO_WINDOW on Windows).
#[cfg(target_os = "windows")]
pub fn adb_command() -> tokio::process::Command {
    let mut std_cmd = std::process::Command::new(local_adb_path());
    use std::os::windows::process::CommandExt;
    std_cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
    tokio::process::Command::from(std_cmd)
}

/// Returns a Tokio Command for the bundled ADB.
#[cfg(not(target_os = "windows"))]
pub fn adb_command() -> tokio::process::Command {
    let std_cmd = std::process::Command::new(local_adb_path());
    tokio::process::Command::from(std_cmd)
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
