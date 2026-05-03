use gemacast_core::network::send_control_message;
use gemacast_core::sender::SenderCommand;
use gemacast_core::types::{ControlMessage, DiscoveredDevice, SenderId};
use std::net::SocketAddr;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use tao::event_loop::EventLoopProxy;
use tokio::task::JoinSet;

use crate::events::DaemonEvent;
use crate::state::DeviceList;

pub fn spawn_discovery_dispatcher(
    set: &mut JoinSet<()>,
    mut combined_rx: tokio::sync::mpsc::Receiver<(ControlMessage, SocketAddr)>,
    state: DeviceList,
    is_broadcasting_for_probe: Arc<AtomicBool>,
    proxy: EventLoopProxy<DaemonEvent>,
    sender_command_tx_for_dispatch: tokio::sync::mpsc::Sender<SenderCommand>,
) {
    set.spawn(async move {
        let device_name = whoami::devicename().unwrap_or_else(|_| "Desktop PC".to_string());
        let sender_id = SenderId(format!("PC_{}", device_name.to_uppercase()));

        while let Some((message, remote_addr)) = combined_rx.recv().await {
            let mut audio_addr = remote_addr;
            audio_addr.set_port(gemacast_core::network::Ports::AUDIO_UDP);

            match message {
                ControlMessage::Probe {
                    device_id: incoming_id,
                } => {
                    if let Some(id) = incoming_id
                        && let Ok(mut map) = state.lock()
                        && let Some(device) = map.get_mut(&id)
                    {
                        device.last_seen = std::time::Instant::now();
                    }

                    let broadcasting = is_broadcasting_for_probe.load(Ordering::Relaxed);

                    let _ = send_control_message(
                        remote_addr.ip(),
                        ControlMessage::Presence {
                            sender_id: sender_id.clone(),
                            sender_name: device_name.clone(),
                            is_offline: !broadcasting,
                            transport: None,
                        },
                    )
                    .await;
                }
                ControlMessage::Connect {
                    device_id,
                    device_name: connect_device_name,
                    transport,
                    ..
                } => {
                    if !is_broadcasting_for_probe.load(Ordering::Relaxed) {
                        let _ = send_control_message(
                            remote_addr.ip(),
                            ControlMessage::Presence {
                                sender_id: sender_id.clone(),
                                sender_name: device_name.clone(),
                                is_offline: true,
                                transport: None,
                            },
                        )
                        .await;
                        continue;
                    }

                    let mut is_new = false;
                    let mut ip_changed = false;
                    let mut old_addr = None;
                    if let Ok(mut map) = state.lock() {
                        if let Some(existing) = map.get(&device_id) {
                            if existing.addr != audio_addr {
                                ip_changed = true;
                                old_addr = Some(existing.addr);
                            }
                        } else {
                            is_new = true;
                        }

                        let device = DiscoveredDevice::from_presence(
                            SenderId(device_id.0.clone()),
                            connect_device_name.clone(),
                            false,
                            audio_addr,
                            transport,
                        );
                        map.insert(device_id.clone(), device);
                    }

                    if ip_changed {
                        if let Some(old) = old_addr {
                            let _ =
                                proxy.send_event(DaemonEvent::DeviceLost(device_id.clone(), old));
                            let _ = sender_command_tx_for_dispatch
                                .send(SenderCommand::RemoveTarget(old))
                                .await;
                        }
                        is_new = true;
                    }

                    if is_new {
                        let _ = proxy.send_event(DaemonEvent::DiscoveredDevice {
                            device_id,
                            name: connect_device_name,
                            addr: audio_addr,
                        });
                    }

                    if !remote_addr.ip().is_loopback() {
                        let _ = sender_command_tx_for_dispatch
                            .send(SenderCommand::AddTarget(audio_addr))
                            .await;
                    }
                }
                ControlMessage::Disconnect { device_id } => {
                    let mut removed_addr = None;
                    if let Ok(mut map) = state.lock()
                        && let Some(removed) = map.remove(&device_id)
                    {
                        removed_addr = Some(removed.addr);
                    }
                    if let Some(addr) = removed_addr {
                        let _ = proxy.send_event(DaemonEvent::DeviceLost(device_id.clone(), addr));
                        if !remote_addr.ip().is_loopback() {
                            let _ = sender_command_tx_for_dispatch
                                .send(SenderCommand::RemoveTarget(addr))
                                .await;
                        }
                    }
                }
                _ => {}
            }
        }
    });
}
