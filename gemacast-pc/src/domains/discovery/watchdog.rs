use gemacast_core::sender::SenderCommand;
use tao::event_loop::EventLoopProxy;
use tokio::task::JoinSet;

use crate::events::DaemonEvent;
use crate::state::DeviceList;

pub fn spawn_stale_device_watchdog(
    set: &mut JoinSet<()>,
    state_for_watchdog: DeviceList,
    proxy_for_watchdog: EventLoopProxy<DaemonEvent>,
    sender_command_tx_for_watchdog: tokio::sync::mpsc::Sender<SenderCommand>,
) {
    set.spawn(async move {
        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(2));
        loop {
            interval.tick().await;
            let mut timed_out = Vec::new();
            if let Ok(mut map) = state_for_watchdog.lock() {
                let now = std::time::Instant::now();
                map.retain(|id, device| {
                    if now.duration_since(device.last_seen).as_secs() > 10
                        && !device.addr.ip().is_loopback()
                    {
                        timed_out.push((id.clone(), device.addr));
                        false
                    } else {
                        true
                    }
                });
            }

            for (id, addr) in timed_out {
                let _ = proxy_for_watchdog.send_event(DaemonEvent::DeviceLost(id, addr));
                let _ = sender_command_tx_for_watchdog
                    .send(SenderCommand::RemoveTarget(addr))
                    .await;
            }
        }
    });
}
