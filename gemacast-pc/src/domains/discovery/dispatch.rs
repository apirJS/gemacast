use gemacast_core::control::http::ControlCommand;
use gemacast_core::control::types::PresenceResponse;
use gemacast_core::stream::sender::AudioStreamCommand;
use gemacast_core::types::{ControlMessage, DeviceId, DiscoveredDevice};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use tao::event_loop::EventLoopProxy;
use tokio::sync::mpsc;
use tokio::task::JoinSet;

use crate::events::DaemonEvent;
use crate::state::DeviceList;

#[allow(clippy::too_many_arguments)]
pub fn spawn_control_dispatcher(
    set: &mut JoinSet<()>,
    mut inbound_control_message_rx: mpsc::Receiver<(ControlMessage, SocketAddr)>,
    mut http_command_rx: mpsc::Receiver<ControlCommand>,
    device_list: DeviceList,
    is_broadcasting: Arc<AtomicBool>,
    proxy: EventLoopProxy<DaemonEvent>,
    audio_engine_command_tx: mpsc::Sender<AudioStreamCommand>,
    sender_id: DeviceId,
    sender_name: String,
    ws_connections: Arc<
        Mutex<HashMap<DeviceId, mpsc::Sender<gemacast_core::control::types::WsEvent>>>,
    >,
) {
    let device_list_for_udp = device_list.clone();

    set.spawn(async move {
        while let Some((message, _remote_addr)) = inbound_control_message_rx.recv().await {
            if let ControlMessage::Probe {
                device_id: incoming_id,
            } = message
                && let Some(id) = incoming_id
                && let Ok(mut map) = device_list_for_udp.lock()
                && let Some(device) = map.get_mut(&id)
            {
                device.last_seen = std::time::Instant::now();
            }
        }
    });

    set.spawn(async move {
        while let Some(cmd) = http_command_rx.recv().await {
            match cmd {
                ControlCommand::Connect {
                    device_id,
                    device_name,
                    source,
                    remote_addr,
                    response_tx,
                } => {
                    let mut audio_addr = remote_addr;
                    audio_addr.set_port(gemacast_core::network::Ports::AUDIO_UDP);

                    register_device(
                        &device_list,
                        &proxy,
                        &audio_engine_command_tx,
                        device_id,
                        device_name,
                        audio_addr,
                        remote_addr,
                        None,
                        source,
                    )
                    .await;

                    let _ = response_tx.send(PresenceResponse {
                        device_id: sender_id.clone(),
                        sender_name: sender_name.clone(),
                        is_offline: false,
                    });
                }
                ControlCommand::Disconnect {
                    device_id,
                    remote_addr: _,
                } => {
                    unregister_device(
                        &device_list,
                        &proxy,
                        &audio_engine_command_tx,
                        device_id,
                        &ws_connections,
                    )
                    .await;
                }
                ControlCommand::GetSources { response_tx } => {
                    #[cfg(target_os = "windows")]
                    let (sources, caps) = get_windows_sources();
                    #[cfg(not(target_os = "windows"))]
                    let (sources, caps) = (
                        vec![gemacast_core::types::AudioSource::Desktop],
                        gemacast_core::types::SenderCapabilities {
                            supports_process_capture: false,
                        },
                    );

                    let _ = response_tx.send(gemacast_core::control::types::SourcesResponse {
                        sources,
                        capabilities: caps,
                    });
                }
                ControlCommand::ChangeSource { device_id, source } => {
                    let _ = audio_engine_command_tx
                        .send(AudioStreamCommand::ChangeSource { device_id, source })
                        .await;
                }
                ControlCommand::Probe {
                    device_id,
                    response_tx,
                } => {
                    if let Some(id) = device_id
                        && let Ok(mut map) = device_list.lock()
                        && let Some(device) = map.get_mut(&id)
                    {
                        device.last_seen = std::time::Instant::now();
                    }

                    let _ = response_tx.send(PresenceResponse {
                        device_id: sender_id.clone(),
                        sender_name: sender_name.clone(),
                        is_offline: !is_broadcasting.load(Ordering::Relaxed),
                    });
                }
            }
        }
    });
}

#[allow(clippy::too_many_arguments)]
async fn register_device(
    device_list: &DeviceList,
    proxy: &EventLoopProxy<DaemonEvent>,
    audio_tx: &mpsc::Sender<AudioStreamCommand>,
    device_id: DeviceId,
    device_name: String,
    audio_addr: SocketAddr,
    remote_addr: SocketAddr,
    transport: Option<gemacast_core::types::TransportType>,
    source: gemacast_core::types::AudioSource,
) {
    let mut is_new = false;
    let mut ip_changed = false;
    let mut old_addr = None;

    if let Ok(mut map) = device_list.lock() {
        if let Some(existing) = map.get(&device_id) {
            if existing.addr != audio_addr {
                ip_changed = true;
                old_addr = Some(existing.addr);
            }
        } else {
            is_new = true;
        }

        let device = DiscoveredDevice::from_presence(
            device_id.clone(),
            device_name.clone(),
            false,
            audio_addr,
            transport,
        );
        map.insert(device_id.clone(), device);
    }

    if ip_changed {
        if let Some(old) = old_addr {
            let _ = proxy.send_event(DaemonEvent::DeviceLost(device_id.clone(), old));
            let _ = audio_tx
                .send(AudioStreamCommand::Unsubscribe {
                    device_id: device_id.clone(),
                })
                .await;
        }
        is_new = true;
    }

    if is_new {
        let _ = proxy.send_event(DaemonEvent::DiscoveredDevice {
            device_id: device_id.clone(),
            name: device_name,
            addr: audio_addr,
        });
    }

    let effective_addr = if remote_addr.ip().is_loopback() {
        None
    } else {
        Some(audio_addr)
    };
    
    let _ = audio_tx
        .send(AudioStreamCommand::Subscribe {
            device_id,
            target_addr: effective_addr,
            source,
        })
        .await;
}

async fn unregister_device(
    device_list: &DeviceList,
    proxy: &EventLoopProxy<DaemonEvent>,
    audio_tx: &mpsc::Sender<AudioStreamCommand>,
    device_id: DeviceId,
    ws_connections: &Arc<
        Mutex<HashMap<DeviceId, mpsc::Sender<gemacast_core::control::types::WsEvent>>>,
    >,
) {
    let mut removed_addr = None;
    if let Ok(mut map) = device_list.lock()
        && let Some(removed) = map.remove(&device_id)
    {
        removed_addr = Some(removed.addr);
    }
    if let Some(addr) = removed_addr {
        let _ = proxy.send_event(DaemonEvent::DeviceLost(device_id.clone(), addr));

        let _ = gemacast_core::control::http::send_ws_event(
            ws_connections,
            &device_id,
            gemacast_core::control::types::WsEvent::Disconnect,
        )
        .await;

        let _ = audio_tx
            .send(AudioStreamCommand::Unsubscribe {
                device_id: device_id.clone(),
            })
            .await;
    }
}

#[cfg(target_os = "windows")]
fn get_windows_sources() -> (
    Vec<gemacast_core::types::AudioSource>,
    gemacast_core::types::SenderCapabilities,
) {
    // Desktop is always available; process capture is supported on Windows
    // but individual processes are discovered dynamically by the CapturePool.
    (
        vec![gemacast_core::types::AudioSource::Desktop],
        gemacast_core::types::SenderCapabilities {
            supports_process_capture: true,
        },
    )
}
