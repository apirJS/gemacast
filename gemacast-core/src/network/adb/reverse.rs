use crate::network::Ports;
use tokio::task::JoinSet;

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
                    let _ = tokio::process::Command::new("adb").args(["reverse", "--remove-all"]).output().await;
                    break;
                }
                _ = interval.tick() => {
                    if let Ok(output) = tokio::process::Command::new("adb").arg("devices").output().await
                        && output.status.success() {
                        let stdout = String::from_utf8_lossy(&output.stdout);
                        for line in stdout.lines() {
                            if line.ends_with("device") {
                                let serial = line.split_whitespace().next().unwrap_or("");
                                if !serial.is_empty()
                                    && let Ok(c) = tokio::process::Command::new("adb")
                                        .args(["-s", serial, "reverse", "--list"])
                                        .output()
                                        .await
                                    {
                                        let check_out = String::from_utf8_lossy(&c.stdout);
                                        if !check_out.contains(&audio_port) {
                                            let _ = tokio::process::Command::new("adb")
                                                .args(["-s", serial, "reverse", &audio_port, &audio_port])
                                                .output()
                                                .await;
                                        }
                                        if !check_out.contains(&discovery_port) {
                                            let _ = tokio::process::Command::new("adb")
                                                .args(["-s", serial, "reverse", &discovery_port, &discovery_port])
                                                .output()
                                                .await;
                                        }
                                        if !check_out.contains(&control_port) {
                                            let _ = tokio::process::Command::new("adb")
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
