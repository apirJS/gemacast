use gemacast_core::network::{AudioSender, SenderCommand};
use gemacast_core::network::{
    DiscoveryBroadcaster, DiscoveryListenerHandles, send_control_message,
};
use gemacast_core::types::{ControlMessage, DiscoveredDevice};
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
                    let _ = proxy.send_event(DaemonEvent::FatalError(format!("{:?}", e)));
                    return;
                }
            };

            let (sender_command_tx, sender_command_rx) =
                tokio::sync::mpsc::channel::<SenderCommand>(32);
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

            let sender_command_tx_for_dispatch = sender_command_tx.clone();
            let state_for_dispatch = state.clone();
            tokio::spawn(async move {
                let mut active_broadcaster_tx: Option<tokio::sync::oneshot::Sender<()>> = None;
                let mut stop_tx_opt = Some(stop_tx);

                while let Some(command) = stream_command_rx.recv().await {
                    match command {
                        StreamCommand::StartBroadcasting => {
                            if active_broadcaster_tx.is_none() {
                                if let Ok(handles) = DiscoveryBroadcaster::new().await {
                                    active_broadcaster_tx = Some(handles.shutdown_tx);
                                    let hostname = "Desktop PC".to_string();
                                    let payload = ControlMessage::Presence {
                                        sender_id: "PC_SENDER_1".to_string(),
                                        sender_name: hostname,
                                        is_offline: false,
                                    };
                                    tokio::spawn(async move {
                                        let _ =
                                            handles.broadcaster.broadcast_presence(payload).await;
                                    });
                                }
                            }
                        }
                        StreamCommand::StopBroadcasting => {
                            if let Some(tx) = active_broadcaster_tx.take() {
                                let _ = tx.send(());
                            }
                        }
                        StreamCommand::AddTarget(target_addr) => {
                            let _ = sender_command_tx
                                .send(SenderCommand::AddTarget(target_addr))
                                .await;
                        }
                        StreamCommand::RemoveTarget(target_addr, device_id) => {
                            let _ = sender_command_tx
                                .send(SenderCommand::RemoveTarget(target_addr))
                                .await;

                            if let Ok(mut map) = state_for_dispatch.lock() {
                                map.remove(&device_id);
                            }

                            let _ = send_control_message(
                                target_addr.ip(),
                                ControlMessage::Disconnect { device_id },
                            )
                            .await;
                        }
                        StreamCommand::StopStream => {
                            active_broadcaster_tx.take();
                            if let Some(tx) = stop_tx_opt.take() {
                                let _ = tx.send(());
                            }
                        }
                    }
                }
            });

            while let Some((message, remote_addr)) = discovery_rx.recv().await {
                let mut audio_addr = remote_addr;
                audio_addr.set_port(gemacast_core::network::AUDIO_PORT);

                match message {
                    ControlMessage::Connect {
                        device_id,
                        device_name,
                    } => {
                        let mut is_new = false;
                        if let Ok(mut map) = state.lock() {
                            is_new = !map.contains_key(&device_id);
                            let device = DiscoveredDevice::from_presence(
                                device_id.clone(),
                                device_name.clone(),
                                false,
                                audio_addr,
                            );
                            map.insert(device_id.clone(), device);
                        }

                        if is_new {
                            let _ = proxy.send_event(DaemonEvent::DiscoveredDevice {
                                device_id,
                                name: device_name,
                                addr: audio_addr,
                            });
                            let _ = sender_command_tx_for_dispatch
                                .send(SenderCommand::AddTarget(audio_addr))
                                .await;
                        }
                    }
                    ControlMessage::Disconnect { device_id } => {
                        if let Ok(mut map) = state.lock() {
                            if let Some(removed) = map.remove(&device_id) {
                                let _ = proxy
                                    .send_event(DaemonEvent::DeviceLost(device_id, removed.addr));
                                let _ = sender_command_tx_for_dispatch
                                    .send(SenderCommand::RemoveTarget(removed.addr))
                                    .await;
                            }
                        }
                    }
                    _ => {}
                }
            }
        });
    });
}
