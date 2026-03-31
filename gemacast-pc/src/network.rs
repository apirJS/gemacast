use gemacast_core::network::{AudioSender, SenderCommand};
use gemacast_core::network::DiscoveryListenerHandles;
use tao::event_loop::EventLoopProxy;

use crate::{
    events::{DaemonEvent, StreamCommand},
    state::DeviceList,
};

pub fn spawn_background_engine(
    proxy: EventLoopProxy<DaemonEvent>,
    state: DeviceList,
    mut stream_command_rx: tokio::sync::mpsc::Receiver<StreamCommand>,
) {
    std::thread::spawn(move || {
        let rt = match tokio::runtime::Runtime::new() {
            Ok(r) => r,
            Err(e) => {
                let _ = proxy.send_event(DaemonEvent::FatalError(e.to_string()));
                return;
            }
        };

        rt.block_on(async {
            let DiscoveryListenerHandles {
                listener,
                mut discovery_rx,
            } = gemacast_core::network::DiscoveryListener::new();

            let engine = match AudioSender::new().await {
                Ok(sender) => sender,
                Err(e) => {
                    let _ = proxy.send_event(DaemonEvent::FatalError(e.to_string()));
                    return;
                }
            };

            let (sender_command_tx, sender_command_rx) = tokio::sync::mpsc::channel::<SenderCommand>(32);
            let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();

            let proxy_for_discovery = proxy.clone();
            tokio::spawn(async move {
                if let Err(e) = listener.start().await {
                    let _ = proxy_for_discovery.send_event(DaemonEvent::FatalError(e.to_string()));
                }
            });

            tokio::spawn(async move {
                let mut engine = engine;
                if let Err(e) = engine.start_broadcast(sender_command_rx, stop_rx).await {
                    eprintln!("Audio stream failed: {:?}", e);
                }
            });

            tokio::spawn(async move {
                let mut stop_tx_opt = Some(stop_tx);
                while let Some(command) = stream_command_rx.recv().await {
                    match command {
                        StreamCommand::AddTarget(target_addr) => {
                            let _ = sender_command_tx.send(SenderCommand::AddTarget(target_addr)).await;
                        }
                        StreamCommand::RemoveTarget(target_addr) => {
                            let _ = sender_command_tx.send(SenderCommand::RemoveTarget(target_addr)).await;
                        }
                        StreamCommand::StopStream => {
                            if let Some(tx) = stop_tx_opt.take() {
                                let _ = tx.send(());
                            }
                        }
                    }
                }
            });

            let state_for_trimmer = state.clone();
            let proxy_for_trimmer = proxy.clone();
            tokio::spawn(async move {
                loop {
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    if let Ok(mut map) = state_for_trimmer.lock() {
                        let now = std::time::Instant::now();
                        let mut to_remove = Vec::new();

                        for (id, dev) in map.iter() {
                            if now.duration_since(dev.last_seen).as_secs() > 3 {
                                to_remove.push((id.clone(), dev.addr));
                            }
                        }

                        for (id, addr) in to_remove {
                            map.remove(&id);
                            let _ = proxy_for_trimmer.send_event(DaemonEvent::DeviceLost(id, addr));
                        }
                    }
                }
            });

            while let Some(device) = discovery_rx.recv().await {
                if device.is_offline {
                    if let Ok(mut map) = state.lock() {
                        if let Some(removed) = map.remove(&device.device_id) {
                            let _ = proxy.send_event(DaemonEvent::DeviceLost(device.device_id, removed.addr));
                        }
                    }
                    continue;
                }

                let mut is_new = false;
                if let Ok(mut map) = state.lock() {
                    is_new = !map.contains_key(&device.device_id);
                    map.insert(device.device_id.clone(), device.clone());
                }

                if is_new {
                    let _ = proxy.send_event(DaemonEvent::DiscoveredDevice {
                        device_id: device.device_id,
                        name: device.device_name,
                        addr: device.addr,
                    });
                }
            }
        });
    });
}
