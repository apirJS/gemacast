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
    let audio_port = format!("tcp:{}", Ports::ADB_AUDIO_TCP);
    let discovery_port = format!("tcp:{}", Ports::ADB_DISCOVERY_TCP);
    let control_port = format!("tcp:{}", Ports::CONTROL);

    set.spawn(async move {
        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(3));
        let mut drop_rx = tcp_drop_tx.subscribe();

        loop {
            tokio::select! {
                _ = drop_rx.recv() => {
                    let _ = adb_command().args(["reverse", "--remove-all"]).output().await;
                    break;
                }
                _ = interval.tick() => {
                    if let Ok(output) = adb_command().arg("devices").output().await
                        && output.status.success() {
                        let stdout = String::from_utf8_lossy(&output.stdout);
                        for line in stdout.lines() {
                            if line.ends_with("device") {
                                let serial = line.split_whitespace().next().unwrap_or("");
                                if !serial.is_empty()
                                    && let Ok(c) = adb_command()
                                        .args(["-s", serial, "reverse", "--list"])
                                        .output()
                                        .await
                                    {
                                        let check_out = String::from_utf8_lossy(&c.stdout);
                                        if !check_out.contains(&audio_port) {
                                            let _ = adb_command()
                                                .args(["-s", serial, "reverse", &audio_port, &audio_port])
                                                .output()
                                                .await;
                                        }
                                        if !check_out.contains(&discovery_port) {
                                            let _ = adb_command()
                                                .args(["-s", serial, "reverse", &discovery_port, &discovery_port])
                                                .output()
                                                .await;
                                        }
                                        if !check_out.contains(&control_port) {
                                            let _ = adb_command()
                                                .args(["-s", serial, "reverse", &control_port, &control_port])
                                                .output()
                                                .await;
                                        }
                                    }
                            }
                        }
                    }
                }
            }
        }
    });
}
