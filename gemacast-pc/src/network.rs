use gemacast_core::network::adb::{
    PresenceProvider, spawn_adb_reverse_watchdog, spawn_audio_spigot, spawn_discovery_spigot,
};
use gemacast_core::network::{AudioSender, SenderCommand};
use gemacast_core::types::{ControlMessage, SenderId};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use tao::event_loop::EventLoopProxy;
use tokio::task::JoinSet;

use crate::{
    domains::{
        audio::{engine::spawn_audio_engine, stream_manager::spawn_stream_command_manager},
        discovery::{
            dispatch::spawn_discovery_dispatcher, udp_listener::spawn_udp_listener,
            watchdog::spawn_stale_device_watchdog,
        },
    },
    events::{DaemonEvent, StreamCommand},
    state::DeviceList,
};

/// Adapter implementing [`PresenceProvider`] for the PC daemon's broadcast state,
/// decoupling the centralized ADB spigots from PC internals.
struct PcPresenceProvider {
    is_broadcasting: Arc<AtomicBool>,
    sender_id: SenderId,
    sender_name: String,
}

impl PresenceProvider for PcPresenceProvider {
    fn is_broadcasting(&self) -> bool {
        self.is_broadcasting.load(Ordering::Relaxed)
    }

    fn sender_id(&self) -> SenderId {
        self.sender_id.clone()
    }

    fn sender_name(&self) -> String {
        self.sender_name.clone()
    }
}

pub fn spawn_background_engine(
    proxy: EventLoopProxy<DaemonEvent>,
    state: DeviceList,
    stream_command_rx: tokio::sync::mpsc::Receiver<StreamCommand>,
) {
    std::thread::spawn(move || {
        let rt = match tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .on_thread_start(|| {
                if let Err(_e) = thread_priority::set_current_thread_priority(
                    thread_priority::ThreadPriority::Max,
                ) {
                    
                }
            })
            .build()
        {
            Ok(r) => r,
            Err(e) => {
                let _ = proxy.send_event(DaemonEvent::FatalError(e.to_string()));
                return;
            }
        };

        rt.block_on(async {
            let mut set = JoinSet::new();
            let (discovery_tx, discovery_rx) = tokio::sync::mpsc::channel(8);
            let listener_res = gemacast_core::network::DiscoveryListener::new(discovery_tx).await;
            let listener = match listener_res {
                Ok(l) => l,
                Err(e) => {
                    let _ = proxy.send_event(DaemonEvent::FatalError(e.to_string()));
                    return;
                }
            };
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
            let (combined_tx, combined_rx) = tokio::sync::mpsc::channel(32);
            let (tcp_drop_tx, _) = tokio::sync::broadcast::channel::<()>(16);
            let (adb_control_tx, _) = tokio::sync::broadcast::channel::<ControlMessage>(16);
            let is_broadcasting = Arc::new(AtomicBool::new(true));

            spawn_udp_listener(
                &mut set,
                listener,
                discovery_rx,
                combined_tx.clone(),
                proxy.clone(),
            );

            let tcp_broadcaster_tx = engine.tcp_broadcaster_tx.clone();

            spawn_audio_engine(&mut set, engine, sender_command_rx, stop_rx);

            let _ = tokio::process::Command::new("adb")
                .arg("kill-server")
                .output()
                .await;
            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;

            spawn_audio_spigot(&mut set, tcp_broadcaster_tx, tcp_drop_tx.clone());

            let device_name = whoami::devicename().unwrap_or_else(|_| "Desktop PC".to_string());
            let sender_id = SenderId(format!("PC_{}", device_name.to_uppercase()));

            let presence_provider = Arc::new(PcPresenceProvider {
                is_broadcasting: is_broadcasting.clone(),
                sender_id,
                sender_name: device_name,
            });

            spawn_discovery_spigot(
                &mut set,
                presence_provider,
                combined_tx.clone(),
                tcp_drop_tx.clone(),
                adb_control_tx.clone(),
            );
            spawn_adb_reverse_watchdog(&mut set, tcp_drop_tx.clone());

            spawn_stale_device_watchdog(
                &mut set,
                state.clone(),
                proxy.clone(),
                sender_command_tx.clone(),
            );

            spawn_stream_command_manager(
                &mut set,
                stream_command_rx,
                crate::domains::audio::stream_manager::StreamManagerContext {
                    is_broadcasting_for_dispatch: is_broadcasting.clone(),
                    tcp_drop_tx: tcp_drop_tx.clone(),
                    state_for_dispatch: state.clone(),
                    proxy_for_dispatch: proxy.clone(),
                    sender_command_tx: sender_command_tx.clone(),
                    stop_tx_opt: Some(stop_tx),
                    adb_control_tx: adb_control_tx.clone(),
                },
            );

            spawn_discovery_dispatcher(
                &mut set,
                combined_rx,
                state.clone(),
                is_broadcasting.clone(),
                proxy.clone(),
                sender_command_tx.clone(),
            );

            while set.join_next().await.is_some() {}
        });
    });
}
