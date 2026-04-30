use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tao::event_loop::EventLoopProxy;
use tokio::task::JoinSet;

use gemacast_core::network::{DiscoveryBroadcaster, send_control_message};

use gemacast_core::types::{ControlMessage, SenderId};

use crate::events::{DaemonCommand, DaemonEvent};
use crate::state::DeviceList;

pub struct StreamManagerContext {
    pub is_broadcasting_for_dispatch: Arc<AtomicBool>,
    pub tcp_drop_tx: tokio::sync::broadcast::Sender<()>,
    pub state_for_dispatch: DeviceList,
    pub proxy_for_dispatch: EventLoopProxy<DaemonEvent>,
    pub stream_command_tx: tokio::sync::mpsc::Sender<gemacast_core::stream::sender::broadcast::StreamCommand>,
    pub stop_tx_opt: Option<tokio::sync::oneshot::Sender<()>>,
    pub adb_control_tx: tokio::sync::broadcast::Sender<ControlMessage>,
}

pub fn spawn_stream_command_manager(
    set: &mut JoinSet<()>,
    mut stream_command_rx: tokio::sync::mpsc::Receiver<DaemonCommand>,
    ctx: StreamManagerContext,
) {
    set.spawn(async move {
        let mut active_broadcaster_tx: Option<tokio::sync::oneshot::Sender<()>> = None;
        let device_name = whoami::devicename().unwrap_or_else(|_| "Desktop PC".to_string());
        let sender_id = SenderId(format!("PC_{}", device_name.to_uppercase()));

        while let Some(command) = stream_command_rx.recv().await {
            match command {
                DaemonCommand::StartBroadcasting => {
                    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
                    if active_broadcaster_tx.is_none()
                        && let Ok(broadcaster) = DiscoveryBroadcaster::new(shutdown_rx).await
                    {
                        ctx.is_broadcasting_for_dispatch
                            .store(true, Ordering::Relaxed);
                        active_broadcaster_tx = Some(shutdown_tx);
                        let state_for_closure = ctx.state_for_dispatch.clone();
                        let sid = sender_id.clone();
                        let sname = device_name.clone();
                        tokio::spawn(async move {
                            let factory = move || {
                                ControlMessage::Presence {
                                    sender_id: sid.clone(),
                                    sender_name: sname.clone(),
                                    is_offline: false,
                                    transport: None,
                                }
                            };
                            let target_ips = move || {
                                if let Ok(map) = state_for_closure.lock() {
                                    map.values()
                                        .filter_map(|d| {
                                            if let std::net::SocketAddr::V4(v4) = d.addr {
                                                Some(std::net::SocketAddrV4::new(
                                                    *v4.ip(),
                                                    gemacast_core::network::Ports::DISCOVERY,
                                                ))
                                            } else {
                                                None
                                            }
                                        })
                                        .collect()
                                } else {
                                    Vec::new()
                                }
                            };
                            let _ = broadcaster.broadcast_presence(factory, target_ips).await;
                        });
                    }
                }
                DaemonCommand::StopBroadcasting => {
                    ctx.is_broadcasting_for_dispatch
                        .store(false, Ordering::Relaxed);
                    if let Some(tx) = active_broadcaster_tx.take() {
                        let _ = tx.send(());
                    }

                    let mut devices_to_remove = Vec::new();
                    if let Ok(map) = ctx.state_for_dispatch.lock() {
                        for (device_id, device) in map.iter() {
                            devices_to_remove.push((device.addr, device_id.clone()));
                        }
                    }

                    for (_addr, _device_id) in devices_to_remove {
                        let _ = ctx
                            .stream_command_tx
                            .send(gemacast_core::stream::sender::broadcast::StreamCommand::Unsubscribe { device_id: _device_id })
                            .await;
                    }
                }
                DaemonCommand::KickDevice(device_id) => {
                    let mut target_addr = None;
                    if let Ok(mut map) = ctx.state_for_dispatch.lock()
                        && let Some(device) = map.remove(&device_id)
                    {
                        target_addr = Some(device.addr);
                    }

                    let _ = ctx
                        .stream_command_tx
                        .send(gemacast_core::stream::sender::broadcast::StreamCommand::Unsubscribe { device_id: device_id.clone() })
                        .await;

                    if let Some(addr) = target_addr {
                        if addr.ip().is_loopback() {
                            let _ = ctx.adb_control_tx.send(ControlMessage::Disconnect { device_id: device_id.clone() });
                        } else {
                            let _ = send_control_message(
                                addr.ip(),
                                ControlMessage::Disconnect { device_id: device_id.clone() },
                            )
                            .await;
                        }
                    }
                }
                DaemonCommand::ChangeBitrate(bitrate) => {
                    let _ = ctx
                        .stream_command_tx
                        .send(gemacast_core::stream::sender::broadcast::StreamCommand::ChangeBitrate(bitrate))
                        .await;
                }
                DaemonCommand::StopAllStreams => {
                    let _ = ctx.tcp_drop_tx.send(());
                    if let Some(tx) = active_broadcaster_tx.take() {
                        let _ = tx.send(());
                    }
                    let _ = ctx.stream_command_tx.send(gemacast_core::stream::sender::broadcast::StreamCommand::Shutdown).await;

                    let mut devices_to_remove = Vec::new();
                    if let Ok(mut map) = ctx.state_for_dispatch.lock() {
                        for (device_id, device) in map.drain() {
                            devices_to_remove.push((device_id, device.addr));
                        }
                    }

                    for (device_id, addr) in devices_to_remove {
                        let _ = ctx
                            .proxy_for_dispatch
                            .send_event(DaemonEvent::DeviceLost(device_id.clone(), addr));
                        let _ = send_control_message(
                            addr.ip(),
                            ControlMessage::Disconnect { device_id },
                        )
                        .await;
                    }
                }
            }
        }
    });
}
