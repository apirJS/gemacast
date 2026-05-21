use gemacast_core::stream::sender::AudioStreamCommand;
use tao::event_loop::EventLoopProxy;
use tokio::task::JoinSet;

use crate::events::DaemonEvent;
use crate::state::DeviceList;

pub fn spawn_stale_device_watchdog(
    set: &mut JoinSet<()>,
    state_for_watchdog: DeviceList,
    proxy_for_watchdog: EventLoopProxy<DaemonEvent>,
    audio_engine_command_tx: tokio::sync::mpsc::Sender<AudioStreamCommand>,
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
                let _ = proxy_for_watchdog.send_event(DaemonEvent::DeviceLost(id.clone(), addr));
                let _ = audio_engine_command_tx
                    .send(AudioStreamCommand::Unsubscribe { device_id: id })
                    .await;
            }
        }
    });
}
