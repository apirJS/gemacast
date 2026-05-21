use gemacast_core::control::http::{ControlCommand, ControlServerState};
use gemacast_core::network::adb::{
    PresenceProvider, spawn_adb_audio_tcp_server, spawn_adb_discovery_tcp_server,
    spawn_adb_port_forwarding_watchdog,
};
use gemacast_core::stream::sender::engine::AudioStreamEngine;
use gemacast_core::types::{ControlMessage, DeviceId};
use std::collections::HashMap;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use tao::event_loop::EventLoopProxy;
use tokio::task::JoinSet;

use crate::{
    domains::{
        audio::{engine::spawn_audio_engine, stream_manager::spawn_stream_command_manager},
        discovery::{
            dispatch::spawn_control_dispatcher, udp_listener::spawn_udp_listener,
            watchdog::spawn_stale_device_watchdog,
        },
    },
    events::{DaemonCommand, DaemonEvent},
    state::DeviceList,
};

struct PcPresenceProvider {
    is_broadcasting: Arc<AtomicBool>,
    sender_id: DeviceId,
    sender_name: String,
}

impl PresenceProvider for PcPresenceProvider {
    fn is_broadcasting(&self) -> bool {
        self.is_broadcasting.load(Ordering::Relaxed)
    }

    fn sender_id(&self) -> DeviceId {
        self.sender_id.clone()
    }

    fn sender_name(&self) -> String {
        self.sender_name.clone()
    }
}

pub fn spawn_background_engine(
    event_loop_proxy: EventLoopProxy<DaemonEvent>,
    device_list: DeviceList,
    daemon_command_rx: tokio::sync::mpsc::Receiver<DaemonCommand>,
) {
    std::thread::spawn(move || {
        let rt = match tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .on_thread_start(|| {
                if let Err(_e) = thread_priority::set_current_thread_priority(
                    thread_priority::ThreadPriority::Max,
                ) {}
            })
            .build()
        {
            Ok(r) => r,
            Err(e) => {
                let _ = event_loop_proxy.send_event(DaemonEvent::FatalError(e.to_string()));
                return;
            }
        };

        rt.block_on(async {
            let mut set = JoinSet::new();
            let (presence_message_tx, presence_message_rx) = tokio::sync::mpsc::channel(8);

            let listener = match gemacast_core::network::PresenceListener::new(presence_message_tx.clone()).await {
                Ok(l) => l,
                Err(e) => {
                    let e_str = e.to_string();
                    let msg = if e_str.contains("Address already in use") || e_str.contains("10048") || e_str.contains("98") || e_str.contains("WSAEADDRINUSE") {
                        "Discovery port is already in use. Is GemaCast already running in the background? Please check your system tray or Task Manager.".to_string()
                    } else {
                        e_str
                    };
                    let _ = event_loop_proxy.send_event(DaemonEvent::FatalError(msg));
                    return;
                }
            };

            let mut engine = AudioStreamEngine::new(true);

            // Channel: dispatcher/watchdog -> audio stream engine (Subscribe, Unsubscribe, ChangeSource, etc.)
            let (audio_engine_command_tx, audio_engine_command_rx) =
                tokio::sync::mpsc::channel::<gemacast_core::stream::sender::AudioStreamCommand>(32);

            // Oneshot: signals the audio engine to shut down gracefully
            let (engine_shutdown_tx, _engine_shutdown_rx) = tokio::sync::oneshot::channel::<()>();

            // Channel: merges all inbound control messages from discovery port and ADB TCP
            let (inbound_control_message_tx, inbound_control_message_rx) = tokio::sync::mpsc::channel(32);

            // Channel: HTTP control commands from Axum server -> dispatcher
            let (http_command_tx, http_command_rx) = tokio::sync::mpsc::channel::<ControlCommand>(32);

            // Broadcast: signals all ADB TCP tasks to tear down their connections
            let (adb_shutdown_signal_tx, _) = tokio::sync::broadcast::channel::<()>(16);

            // Broadcast: sends control messages outbound to ADB-connected clients
            let (adb_outbound_control_tx, _) = tokio::sync::broadcast::channel::<ControlMessage>(16);

            let is_broadcasting = Arc::new(AtomicBool::new(true));

            // Channel: background tasks -> main thread for fatal error reporting
            let (fatal_error_tx, mut fatal_error_rx) = tokio::sync::mpsc::channel::<String>(8);
            let event_loop_proxy_clone = event_loop_proxy.clone();

            set.spawn(async move {
                while let Some(msg) = fatal_error_rx.recv().await {
                    let _ = event_loop_proxy_clone.send_event(DaemonEvent::FatalError(msg));
                }
            });
            
            let desktop_audio_broadcast_tx = match engine.pool.subscribe(gemacast_core::types::AudioSource::Desktop, None).await {
                Ok(tx) => tx,
                Err(e) => {
                    let _ = event_loop_proxy.send_event(DaemonEvent::FatalError(format!("Failed to initialize desktop audio capture: {}", e)));
                    return;
                }
            };

            // Seed the watch channel with the initial Desktop broadcast sender
            let tcp_source_watch_rx = engine.tcp_source_watch();
            engine.seed_tcp_source(desktop_audio_broadcast_tx);

            let _ = tokio::process::Command::new("adb")
                .arg("kill-server")
                .output()
                .await;
            
            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;

            let device_name = whoami::devicename().unwrap_or_else(|_| "Desktop PC".to_string());
            let sender_id = DeviceId::new();
            let presence_provider = Arc::new(PcPresenceProvider {
                is_broadcasting: is_broadcasting.clone(),
                sender_id: sender_id.clone(),
                sender_name: device_name.clone(),
            });

            let ws_connections = Arc::new(Mutex::new(HashMap::new()));

            let control_server_state = ControlServerState {
                command_tx: http_command_tx,
                is_broadcasting: is_broadcasting.clone(),
                sender_id: sender_id.clone(),
                sender_name: device_name.clone(),
                ws_connections: ws_connections.clone(),
            };

            let (_control_server_shutdown_tx, control_server_shutdown_rx) =
                tokio::sync::oneshot::channel::<()>();

            let event_proxy_for_server = event_loop_proxy.clone();
            set.spawn(async move {
                if let Err(e) = gemacast_core::control::start_control_server(
                    control_server_state,
                    control_server_shutdown_rx,
                )
                .await
                {
                    let e_str = e.to_string();
                    let msg = if e_str.contains("Address already in use") || e_str.contains("10048") {
                        "Control port (55559) is already in use. Is GemaCast already running?".to_string()
                    } else {
                        format!("Control server failed: {}", e_str)
                    };
                    let _ = event_proxy_for_server.send_event(DaemonEvent::FatalError(msg));
                }
            });

            spawn_udp_listener(
                &mut set,
                listener,
                presence_message_rx,
                inbound_control_message_tx.clone(),
                event_loop_proxy.clone(),
            );

            spawn_audio_engine(&mut set, engine, audio_engine_command_rx, event_loop_proxy.clone());

            spawn_adb_audio_tcp_server(&mut set, tcp_source_watch_rx, adb_shutdown_signal_tx.clone(), fatal_error_tx.clone());

            spawn_adb_discovery_tcp_server(
                &mut set,
                presence_provider,
                inbound_control_message_tx.clone(),
                adb_shutdown_signal_tx.clone(),
                adb_outbound_control_tx.clone(),
                fatal_error_tx.clone(),
            );
            spawn_adb_port_forwarding_watchdog(&mut set, adb_shutdown_signal_tx.clone());

            spawn_stale_device_watchdog(
                &mut set,
                device_list.clone(),
                event_loop_proxy.clone(),
                audio_engine_command_tx.clone(),
            );

            spawn_stream_command_manager(
                &mut set,
                daemon_command_rx,
                crate::domains::audio::stream_manager::StreamManagerContext {
                    is_broadcasting_for_dispatch: is_broadcasting.clone(),
                    adb_shutdown_signal_tx: adb_shutdown_signal_tx.clone(),
                    device_list_for_dispatch: device_list.clone(),
                    event_loop_proxy_for_dispatch: event_loop_proxy.clone(),
                    audio_engine_command_tx: audio_engine_command_tx.clone(),
                    engine_shutdown_tx: Some(engine_shutdown_tx),
                    adb_outbound_control_tx: adb_outbound_control_tx.clone(),
                    ws_connections: ws_connections.clone(),
                },
            );

            spawn_control_dispatcher(
                &mut set,
                inbound_control_message_rx,
                http_command_rx,
                device_list.clone(),
                is_broadcasting.clone(),
                event_loop_proxy.clone(),
                audio_engine_command_tx.clone(),
                sender_id,
                device_name,
                ws_connections.clone(),
            );

            while set.join_next().await.is_some() {}
        });
    });
}
