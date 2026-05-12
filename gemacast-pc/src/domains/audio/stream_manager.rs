use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tao::event_loop::EventLoopProxy;
use tokio::task::JoinSet;

use gemacast_core::control::HttpControlClient;
use gemacast_core::control::types::WsEvent;
use gemacast_core::discovery::PresenceBroadcaster;

use gemacast_core::types::{ControlMessage, DeviceId};

use crate::events::{DaemonCommand, DaemonEvent};
use crate::state::DeviceList;

pub struct StreamManagerContext {
    pub is_broadcasting_for_dispatch: Arc<AtomicBool>,
    pub adb_shutdown_signal_tx: tokio::sync::broadcast::Sender<()>,
    pub device_list_for_dispatch: DeviceList,
    pub event_loop_proxy_for_dispatch: EventLoopProxy<DaemonEvent>,
    pub audio_engine_command_tx:
        tokio::sync::mpsc::Sender<gemacast_core::stream::sender::AudioStreamCommand>,
    pub engine_shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
    pub adb_outbound_control_tx: tokio::sync::broadcast::Sender<ControlMessage>,
    pub ws_connections: Arc<Mutex<HashMap<DeviceId, tokio::sync::mpsc::Sender<WsEvent>>>>,
}

pub fn spawn_stream_command_manager(
    set: &mut JoinSet<()>,
    mut daemon_command_rx: tokio::sync::mpsc::Receiver<DaemonCommand>,
    ctx: StreamManagerContext,
) {
    set.spawn(async move {
        let mut broadcaster_shutdown_tx: Option<tokio::sync::oneshot::Sender<()>> = None;
        let device_name = whoami::devicename().unwrap_or_else(|_| "Desktop PC".to_string());
        let sender_id = DeviceId(format!("PC_{}", device_name.to_uppercase()));

        while let Some(command) = daemon_command_rx.recv().await {
            match command {
                DaemonCommand::StartBroadcasting => {
                    let (broadcaster_stop_tx, broadcaster_stop_rx) =
                        tokio::sync::oneshot::channel();
                    if broadcaster_shutdown_tx.is_none()
                        && let Ok(broadcaster) = PresenceBroadcaster::new(broadcaster_stop_rx).await
                    {
                        ctx.is_broadcasting_for_dispatch
                            .store(true, Ordering::Relaxed);

                        broadcaster_shutdown_tx = Some(broadcaster_stop_tx);

                        let device_list_for_closure = ctx.device_list_for_dispatch.clone();
                        let sid = sender_id.clone();
                        let sname = device_name.clone();

                        tokio::spawn(async move {
                            let factory = move || ControlMessage::Presence {
                                device_id: sid.clone(),
                                sender_name: sname.clone(),
                                is_offline: false,
                                transport: None,
                            };
                            let target_ips = move || {
                                if let Ok(map) = device_list_for_closure.lock() {
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
                            let _ = broadcaster.run_broadcast_loop(factory, target_ips).await;
                        });
                    }
                }
                DaemonCommand::StopBroadcasting => {
                    ctx.is_broadcasting_for_dispatch
                        .store(false, Ordering::Relaxed);
                    if let Some(tx) = broadcaster_shutdown_tx.take() {
                        let _ = tx.send(());
                    }

                    let mut devices_to_remove = Vec::new();
                    if let Ok(map) = ctx.device_list_for_dispatch.lock() {
                        for (device_id, device) in map.iter() {
                            devices_to_remove.push((device.addr, device_id.clone()));
                        }
                    }

                    for (_addr, _device_id) in devices_to_remove {
                        let _ = ctx
                            .audio_engine_command_tx
                            .send(
                                gemacast_core::stream::sender::AudioStreamCommand::Unsubscribe {
                                    device_id: _device_id,
                                },
                            )
                            .await;
                    }
                }
                DaemonCommand::KickDevice(device_id) => {
                    let mut target_addr = None;
                    if let Ok(mut map) = ctx.device_list_for_dispatch.lock()
                        && let Some(device) = map.remove(&device_id)
                    {
                        target_addr = Some(device.addr);
                    }

                    let _ = ctx
                        .audio_engine_command_tx
                        .send(
                            gemacast_core::stream::sender::AudioStreamCommand::Unsubscribe {
                                device_id: device_id.clone(),
                            },
                        )
                        .await;

                    let ws_sent = gemacast_core::control::http::send_ws_event(
                        &ctx.ws_connections,
                        &device_id,
                        WsEvent::Disconnect,
                    )
                    .await
                    .is_ok();

                    if !ws_sent && let Some(addr) = target_addr {
                        if addr.ip().is_loopback() {
                            let _ = ctx
                                .adb_outbound_control_tx
                                .send(ControlMessage::Disconnect {
                                    device_id: device_id.clone(),
                                });
                        } else {
                            let client = HttpControlClient::new(addr.ip());
                            let _ = client.send_disconnect_request(device_id.clone()).await;
                        }
                    }
                }
                DaemonCommand::StopAllStreams => {
                    let _ = ctx.adb_shutdown_signal_tx.send(());
                    if let Some(tx) = broadcaster_shutdown_tx.take() {
                        let _ = tx.send(());
                    }
                    let _ = ctx
                        .audio_engine_command_tx
                        .send(gemacast_core::stream::sender::AudioStreamCommand::Shutdown)
                        .await;

                    let mut devices_to_remove = Vec::new();
                    if let Ok(mut map) = ctx.device_list_for_dispatch.lock() {
                        for (device_id, device) in map.drain() {
                            devices_to_remove.push((device_id, device.addr));
                        }
                    }

                    for (device_id, addr) in devices_to_remove {
                        let _ = ctx
                            .event_loop_proxy_for_dispatch
                            .send_event(DaemonEvent::DeviceLost(device_id.clone(), addr));

                        if addr.ip().is_loopback() {
                            let _ = ctx
                                .adb_outbound_control_tx
                                .send(ControlMessage::Disconnect { device_id });
                        } else {
                            let client = HttpControlClient::new(addr.ip());
                            let _ = client.send_disconnect_request(device_id).await;
                        }
                    }
                }
            }
        }
    });
}
