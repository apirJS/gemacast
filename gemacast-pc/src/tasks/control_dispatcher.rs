//! Routes inbound control commands (from HTTP and UDP) to the appropriate handlers.
//!
//! Spawns two tasks:
//! 1. **Probe heartbeat handler**: Updates `last_seen` for devices sending UDP probes.
//! 2. **HTTP command handler**: Processes [`ControlCommand`]s from the Axum control server
//!    (connect, disconnect, change source, change bitrate, get sources, probe).

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use gemacast_core::control::http::ControlCommand;
use gemacast_core::control::types::PresenceResponse;
use gemacast_core::types::{ControlMessage, DeviceId, DiscoveredDevice};
use tokio::sync::mpsc;
use tokio::task::JoinSet;

use crate::traits::{
    AudioController, DeviceNotifier, DeviceRegistry, RegistrationOutcome, TrayNotifier,
};

/// Shared context for the control dispatcher.
///
/// Groups all the trait dependencies and identity info needed to handle
/// HTTP control commands. Extracted as a struct to avoid a 10-parameter
/// function signature.
pub struct ControlDispatcher {
    pub registry: Arc<dyn DeviceRegistry>,
    pub tray: Arc<dyn TrayNotifier>,
    pub audio: Arc<dyn AudioController>,
    pub notifier: Arc<dyn DeviceNotifier>,
    pub sender_id: DeviceId,
    pub sender_name: String,
    pub is_broadcasting: Arc<AtomicBool>,
}

impl ControlDispatcher {
    /// Handle a single HTTP control command.
    ///
    /// Extracted from the receive loop for unit testing.
    pub async fn handle_http_command(&self, cmd: ControlCommand) {
        match cmd {
            ControlCommand::Connect {
                device_id,
                device_name,
                source,
                remote_addr,
                bitrate,
                response_tx,
            } => {
                tracing::info!(
                    "ControlCommand::Connect from {:?} at {}",
                    device_id,
                    remote_addr
                );
                let mut audio_addr = remote_addr;
                audio_addr.set_port(gemacast_core::network::Ports::AUDIO_UDP);

                register_device(
                    self.registry.as_ref(),
                    self.tray.as_ref(),
                    self.audio.as_ref(),
                    device_id,
                    device_name,
                    audio_addr,
                    remote_addr,
                    None,
                    source,
                    bitrate,
                )
                .await;

                let _ = response_tx.send(PresenceResponse {
                    device_id: self.sender_id.clone(),
                    sender_name: self.sender_name.clone(),
                    is_offline: false,
                });
            }
            ControlCommand::Disconnect {
                device_id,
                remote_addr: _,
            } => {
                tracing::info!("ControlCommand::Disconnect from {:?}", device_id);
                unregister_device(
                    self.registry.as_ref(),
                    self.tray.as_ref(),
                    self.audio.as_ref(),
                    self.notifier.as_ref(),
                    device_id,
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
                tracing::info!(
                    "ControlCommand::ChangeSource for {:?} to {:?}",
                    device_id,
                    source
                );
                self.audio.change_source(device_id, source).await;
            }
            ControlCommand::ChangeBitrate { device_id, bitrate } => {
                tracing::info!(
                    "ControlCommand::ChangeBitrate for {:?} to {:?}",
                    device_id,
                    bitrate
                );
                self.audio.change_bitrate(device_id, bitrate).await;
            }
            ControlCommand::Probe {
                device_id,
                response_tx,
            } => {
                if let Some(id) = device_id {
                    self.registry.update_last_seen(&id);
                }

                let _ = response_tx.send(PresenceResponse {
                    device_id: self.sender_id.clone(),
                    sender_name: self.sender_name.clone(),
                    is_offline: !self.is_broadcasting.load(Ordering::Relaxed),
                });
            }
        }
    }
}

/// Spawn the control dispatcher tasks.
pub fn spawn_control_dispatcher(
    set: &mut JoinSet<()>,
    mut inbound_control_rx: mpsc::Receiver<(ControlMessage, SocketAddr)>,
    mut http_command_rx: mpsc::Receiver<ControlCommand>,
    dispatcher: Arc<ControlDispatcher>,
    registry_for_probes: Arc<dyn DeviceRegistry>,
) {
    // Task 1: Handle UDP probe heartbeats (just update last_seen)
    set.spawn(async move {
        while let Some((message, _remote_addr)) = inbound_control_rx.recv().await {
            if let ControlMessage::Probe {
                device_id: Some(id),
            } = message
            {
                registry_for_probes.update_last_seen(&id);
            }
        }
    });

    // Task 2: Handle HTTP control commands
    set.spawn(async move {
        while let Some(cmd) = http_command_rx.recv().await {
            dispatcher.handle_http_command(cmd).await;
        }
    });
}

// ---------------------------------------------------------------------------
// Device registration / unregistration
// ---------------------------------------------------------------------------

/// Register a device: update the registry, notify the tray, and subscribe to audio.
///
/// Handles three cases:
/// - **New device**: Notify tray, subscribe to audio.
/// - **IP changed**: Notify tray of loss at old IP, unsubscribe old, then treat as new.
/// - **Already registered**: Just ensure audio subscription is active.
#[allow(clippy::too_many_arguments)]
pub async fn register_device(
    registry: &dyn DeviceRegistry,
    tray: &dyn TrayNotifier,
    audio: &dyn AudioController,
    device_id: DeviceId,
    device_name: String,
    audio_addr: SocketAddr,
    remote_addr: SocketAddr,
    transport: Option<gemacast_core::types::TransportType>,
    source: Option<gemacast_core::types::AudioSource>,
    bitrate: Option<i32>,
) {
    tracing::debug!(
        "Registering device: {} ({:?}) at {}",
        device_name,
        device_id,
        audio_addr
    );

    let device = DiscoveredDevice::from_presence(
        device_id.clone(),
        device_name.clone(),
        false,
        audio_addr,
        transport,
    );

    let outcome = registry.register(device);

    match outcome {
        RegistrationOutcome::AddressChanged { old_addr } => {
            tray.notify_device_lost(device_id.clone(), old_addr);
            audio.unsubscribe(&device_id).await;
            tray.notify_device_discovered(device_id.clone(), device_name, audio_addr, transport);
        }
        RegistrationOutcome::NewDevice => {
            tray.notify_device_discovered(device_id.clone(), device_name, audio_addr, transport);
        }
        RegistrationOutcome::AlreadyRegistered => {
            // No tray notification needed — device is already shown.
        }
    }

    // ADB/TCP devices use None (audio goes through the TCP tunnel, not UDP)
    let effective_addr = if remote_addr.ip().is_loopback() {
        None
    } else {
        Some(audio_addr)
    };

    audio
        .subscribe(device_id, effective_addr, source, bitrate)
        .await;
}

/// Unregister a device: remove from registry, notify tray, disconnect via WS, unsubscribe audio.
pub async fn unregister_device(
    registry: &dyn DeviceRegistry,
    tray: &dyn TrayNotifier,
    audio: &dyn AudioController,
    notifier: &dyn DeviceNotifier,
    device_id: DeviceId,
) {
    tracing::debug!("Unregistering device: {:?}", device_id);

    let Some(removed) = registry.unregister(&device_id) else {
        return;
    };

    tray.notify_device_lost(device_id.clone(), removed.addr);
    notifier
        .notify_disconnect(&device_id, Some(removed.addr))
        .await;
    audio.unsubscribe(&device_id).await;
}

#[cfg(target_os = "windows")]
fn get_windows_sources() -> (
    Vec<gemacast_core::types::AudioSource>,
    gemacast_core::types::SenderCapabilities,
) {
    (
        vec![gemacast_core::types::AudioSource::Desktop],
        gemacast_core::types::SenderCapabilities {
            supports_process_capture: true,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::mocks::*;

    fn make_addr(s: &str) -> SocketAddr {
        s.parse().unwrap()
    }

    #[tokio::test]
    async fn register_should_notify_tray_for_new_device() {
        let registry = MockDeviceRegistry::new();
        let tray = MockTrayNotifier::new();
        let audio = MockAudioController::new();

        register_device(
            &registry,
            &tray,
            &audio,
            DeviceId("phone-1".into()),
            "My Phone".into(),
            make_addr("192.168.1.5:9000"),
            make_addr("192.168.1.5:55559"),
            None,
            None,
            None,
        )
        .await;

        let tray_calls = tray.take_calls();
        assert_eq!(tray_calls.len(), 1);
        assert!(
            matches!(&tray_calls[0], TrayCall::Discovered { device_id, name, .. } if device_id.0 == "phone-1" && name == "My Phone")
        );

        let audio_calls = audio.take_calls();
        assert_eq!(audio_calls.len(), 1);
        assert!(
            matches!(&audio_calls[0], AudioCall::Subscribe { device_id, .. } if device_id.0 == "phone-1")
        );
    }

    #[tokio::test]
    async fn register_should_handle_ip_change_correctly() {
        let registry = MockDeviceRegistry::with_device("phone-1", "192.168.1.1:9000");
        let tray = MockTrayNotifier::new();
        let audio = MockAudioController::new();

        register_device(
            &registry,
            &tray,
            &audio,
            DeviceId("phone-1".into()),
            "My Phone".into(),
            make_addr("192.168.1.2:9000"), // new IP!
            make_addr("192.168.1.2:55559"),
            None,
            None,
            None,
        )
        .await;

        let tray_calls = tray.take_calls();
        // Should get: Lost (old IP) + Discovered (new IP)
        assert_eq!(tray_calls.len(), 2);
        assert!(
            matches!(&tray_calls[0], TrayCall::Lost { device_id, addr } if device_id.0 == "phone-1" && *addr == make_addr("192.168.1.1:9000"))
        );
        assert!(
            matches!(&tray_calls[1], TrayCall::Discovered { device_id, .. } if device_id.0 == "phone-1")
        );

        let audio_calls = audio.take_calls();
        // Should get: Unsubscribe (old) + Subscribe (new)
        assert_eq!(audio_calls.len(), 2);
        assert!(
            matches!(&audio_calls[0], AudioCall::Unsubscribe { device_id } if device_id.0 == "phone-1")
        );
        assert!(
            matches!(&audio_calls[1], AudioCall::Subscribe { device_id, .. } if device_id.0 == "phone-1")
        );
    }

    #[tokio::test]
    async fn register_should_use_none_addr_for_loopback() {
        let registry = MockDeviceRegistry::new();
        let tray = MockTrayNotifier::new();
        let audio = MockAudioController::new();

        register_device(
            &registry,
            &tray,
            &audio,
            DeviceId("adb-dev".into()),
            "ADB Phone".into(),
            make_addr("127.0.0.1:9000"),
            make_addr("127.0.0.1:55559"), // loopback → ADB mode
            None,
            None,
            None,
        )
        .await;

        let audio_calls = audio.take_calls();
        assert_eq!(audio_calls.len(), 1);
        assert!(matches!(
            &audio_calls[0],
            AudioCall::Subscribe {
                target_addr: None,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn register_should_not_re_notify_for_existing_device_at_same_addr() {
        let registry = MockDeviceRegistry::with_device("phone-1", "192.168.1.1:9000");
        let tray = MockTrayNotifier::new();
        let audio = MockAudioController::new();

        register_device(
            &registry,
            &tray,
            &audio,
            DeviceId("phone-1".into()),
            "My Phone".into(),
            make_addr("192.168.1.1:9000"), // same addr
            make_addr("192.168.1.1:55559"),
            None,
            None,
            None,
        )
        .await;

        // No tray notification for existing device at same addr
        assert!(tray.take_calls().is_empty());
        // Audio subscribe still sent (idempotent)
        assert_eq!(audio.take_calls().len(), 1);
    }

    #[tokio::test]
    async fn unregister_should_notify_tray_and_unsubscribe() {
        let registry = MockDeviceRegistry::with_device("phone-1", "192.168.1.1:9000");
        let tray = MockTrayNotifier::new();
        let audio = MockAudioController::new();
        let notifier = MockDeviceNotifier::new();

        unregister_device(
            &registry,
            &tray,
            &audio,
            &notifier,
            DeviceId("phone-1".into()),
        )
        .await;

        let tray_calls = tray.take_calls();
        assert_eq!(tray_calls.len(), 1);
        assert!(
            matches!(&tray_calls[0], TrayCall::Lost { device_id, .. } if device_id.0 == "phone-1")
        );

        let audio_calls = audio.take_calls();
        assert_eq!(audio_calls.len(), 1);
        assert!(
            matches!(&audio_calls[0], AudioCall::Unsubscribe { device_id } if device_id.0 == "phone-1")
        );

        assert!(!registry.contains("phone-1"));
    }

    #[tokio::test]
    async fn unregister_should_do_nothing_for_unknown_device() {
        let registry = MockDeviceRegistry::new();
        let tray = MockTrayNotifier::new();
        let audio = MockAudioController::new();
        let notifier = MockDeviceNotifier::new();

        unregister_device(
            &registry,
            &tray,
            &audio,
            &notifier,
            DeviceId("ghost".into()),
        )
        .await;

        assert!(tray.take_calls().is_empty());
        assert!(audio.take_calls().is_empty());
    }
}
