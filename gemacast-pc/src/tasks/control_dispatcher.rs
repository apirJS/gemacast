//! Routes inbound control commands (from HTTP and UDP) to the appropriate handlers.
//!
//! Spawns two tasks:
//! 1. **Probe heartbeat handler**: Updates `last_seen` for devices sending UDP probes.
//! 2. **HTTP command handler**: Processes [`ControlCommand`]s from the Axum control server
//!    (connect, disconnect, change source, change bitrate, get sources, probe).

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use gemacast_core::control::SessionAuthorizer;
use gemacast_core::control::http::ControlCommand;
use gemacast_core::control::messages::ControlMessage;
use gemacast_core::control::types::PresenceResponse;
use gemacast_core::domain::types::{DeviceId, DiscoveredDevice};
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
    pub authorizer: SessionAuthorizer,
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
                authorized,
                pending_request_id,
            } => {
                tracing::info!(
                    "ControlCommand::Connect from {:?} at {}",
                    device_id.clone(),
                    remote_addr
                );
                let mut audio_addr = remote_addr;
                audio_addr.set_port(gemacast_core::network::Ports::AUDIO_UDP);

                let approved_request_id = if authorized {
                    None
                } else if let Some(request_id) = pending_request_id {
                    match self.authorizer.pending_status(&request_id, &device_id) {
                        Some(gemacast_core::control::PendingApprovalStatus::Pending) => {
                            let _ = response_tx.send(Ok(PresenceResponse {
                                device_id: self.sender_id.clone(),
                                sender_name: self.sender_name.clone(),
                                is_offline: false,
                                pc_network_link: None,
                                device_registered: Some(false),
                                session_token: None,
                                session_generation: None,
                                pending_request_id: Some(request_id),
                            }));
                            return;
                        }
                        Some(gemacast_core::control::PendingApprovalStatus::Approved) => {
                            Some(request_id)
                        }
                        Some(gemacast_core::control::PendingApprovalStatus::Rejected) => {
                            self.authorizer.remove_pending(&request_id);
                            let _ = response_tx.send(Err(format!(
                                "connection request {request_id} was rejected on the PC"
                            )));
                            return;
                        }
                        None => {
                            let _ = response_tx.send(Err(format!(
                                "connection request {request_id} is invalid or expired"
                            )));
                            return;
                        }
                    }
                } else {
                    let request_id = match self.authorizer.create_pending(device_id.clone()) {
                        Ok(request_id) => request_id,
                        Err(error) => {
                            let _ = response_tx.send(Err(error));
                            return;
                        }
                    };
                    let tray = self.tray.clone();
                    let authorizer = self.authorizer.clone();
                    let approval_device_id = device_id.clone();
                    let approval_device_name = device_name.clone();
                    let approval_request_id = request_id.clone();
                    tokio::spawn(async move {
                        let approved = tray
                            .request_connection_approval(
                                approval_request_id.clone(),
                                approval_device_id,
                                approval_device_name,
                                remote_addr,
                            )
                            .await;
                        authorizer.resolve_pending(&approval_request_id, approved);
                    });
                    let _ = response_tx.send(Ok(PresenceResponse {
                        device_id: self.sender_id.clone(),
                        sender_name: self.sender_name.clone(),
                        is_offline: false,
                        pc_network_link: None,
                        device_registered: Some(false),
                        session_token: None,
                        session_generation: None,
                        pending_request_id: Some(request_id),
                    }));
                    return;
                };

                let pending_session = match self.authorizer.prepare(device_id.clone()) {
                    Ok(session) => session,
                    Err(error) => {
                        let _ = response_tx.send(Err(error));
                        return;
                    }
                };
                let generation = pending_session.generation().0;
                let result = register_device(
                    self.registry.as_ref(),
                    self.tray.as_ref(),
                    self.audio.as_ref(),
                    device_id.clone(),
                    generation,
                    device_name,
                    audio_addr,
                    remote_addr,
                    None,
                    source,
                    bitrate,
                )
                .await;

                let result = result.and_then(|()| {
                    let (session_token, session_generation) =
                        self.authorizer.commit(pending_session)?;
                    Ok(PresenceResponse {
                        device_id: self.sender_id.clone(),
                        sender_name: self.sender_name.clone(),
                        is_offline: false,
                        pc_network_link: None,
                        device_registered: Some(true),
                        session_token: Some(session_token),
                        session_generation: Some(session_generation),
                        pending_request_id: approved_request_id.clone(),
                    })
                });
                if let Some(request_id) = approved_request_id {
                    self.authorizer.remove_pending(&request_id);
                }
                let _ = response_tx.send(result);
            }
            ControlCommand::Disconnect {
                device_id,
                remote_addr: _,
                generation,
                response_tx,
            } => {
                tracing::info!("ControlCommand::Disconnect from {:?}", device_id);
                if generation
                    .is_some_and(|generation| !self.authorizer.is_current(&device_id, generation))
                {
                    let _ = response_tx.send(Ok(()));
                    return;
                }
                let result = unregister_device(
                    self.registry.as_ref(),
                    self.tray.as_ref(),
                    self.audio.as_ref(),
                    self.notifier.as_ref(),
                    device_id.clone(),
                )
                .await;
                if result.is_ok() {
                    self.authorizer.revoke(&device_id, generation);
                }
                let _ = response_tx.send(result);
            }
            ControlCommand::GetSources { response_tx } => {
                let (sources, caps) = get_platform_sources();

                let _ = response_tx.send(gemacast_core::control::types::SourcesResponse {
                    sources,
                    capabilities: caps,
                });
            }
            ControlCommand::ChangeSource {
                device_id,
                source,
                response_tx,
            } => {
                tracing::info!(
                    "ControlCommand::ChangeSource for {:?} to {:?}",
                    device_id,
                    source
                );
                let _ = response_tx.send(self.audio.change_source(device_id, source).await);
            }
            ControlCommand::ChangeBitrate {
                device_id,
                bitrate,
                response_tx,
            } => {
                tracing::info!(
                    "ControlCommand::ChangeBitrate for {:?} to {:?}",
                    device_id,
                    bitrate
                );
                let _ = response_tx.send(self.audio.change_bitrate(device_id, bitrate).await);
            }
            ControlCommand::Probe {
                device_id,
                response_tx,
            } => {
                // `update_last_seen` refreshes only an existing entry, so its
                // answer is the registration status *before* this probe — a
                // probe cannot resurrect a device the watchdog already evicted.
                // A probe with no device_id carries no per-device claim.
                let device_registered = device_id.map(|id| self.registry.update_last_seen(&id));

                let _ = response_tx.send(PresenceResponse {
                    device_id: self.sender_id.clone(),
                    sender_name: self.sender_name.clone(),
                    is_offline: !self.is_broadcasting.load(Ordering::Relaxed),
                    pc_network_link: None,
                    device_registered,
                    session_token: None,
                    session_generation: None,
                    pending_request_id: None,
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
                // UDP heartbeats are fire-and-forget: there is no response
                // channel to carry the registration status back.
                let _ = registry_for_probes.update_last_seen(&id);
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
    generation: u64,
    device_name: String,
    audio_addr: SocketAddr,
    remote_addr: SocketAddr,
    transport: Option<gemacast_core::domain::types::TransportType>,
    source: Option<gemacast_core::domain::types::AudioSource>,
    bitrate: Option<i32>,
) -> Result<(), String> {
    tracing::debug!(
        "Registering device: {} ({:?}) at {}",
        device_name,
        device_id,
        audio_addr
    );

    // ADB/TCP devices use None (audio goes through the TCP tunnel, not UDP)
    let effective_addr = if remote_addr.ip().is_loopback() {
        None
    } else {
        Some(audio_addr)
    };

    if let Err(error) = audio
        .subscribe(
            device_id.clone(),
            generation,
            effective_addr,
            source,
            bitrate,
        )
        .await
    {
        tracing::warn!("Audio subscription failed: {error}");
        return Err(error);
    }

    let device = DiscoveredDevice::from_presence(
        device_id.clone(),
        device_name.clone(),
        false,
        audio_addr,
        transport,
    );
    match registry.register(device) {
        RegistrationOutcome::AddressChanged { old_addr } => {
            tray.notify_device_lost(device_id.clone(), old_addr);
            tray.notify_device_discovered(device_id, device_name, audio_addr, transport);
        }
        RegistrationOutcome::NewDevice => {
            tray.notify_device_discovered(device_id, device_name, audio_addr, transport);
        }
        RegistrationOutcome::AlreadyRegistered => {}
    }
    Ok(())
}

/// Unregister a device: remove from registry, notify tray, disconnect via WS, unsubscribe audio.
pub async fn unregister_device(
    registry: &dyn DeviceRegistry,
    tray: &dyn TrayNotifier,
    audio: &dyn AudioController,
    notifier: &dyn DeviceNotifier,
    device_id: DeviceId,
) -> Result<(), String> {
    tracing::debug!("Unregistering device: {:?}", device_id);

    if let Some(removed) = registry.unregister(&device_id) {
        tray.notify_device_lost(device_id.clone(), removed.addr);
        notifier
            .notify_disconnect(&device_id, Some(removed.addr))
            .await;
    }
    audio.unsubscribe(&device_id).await
}

/// Returns the available audio sources and sender capabilities for the current platform.
///
/// - **Windows**: Always supports process capture (via WASAPI).
/// - **Linux**: Supports process capture only if PipeWire is available.
/// - **macOS**: Always supports process capture (via ScreenCaptureKit).
/// - **Other**: Desktop capture only.
fn get_platform_sources() -> (
    Vec<gemacast_core::domain::types::AudioSource>,
    gemacast_core::domain::types::SenderCapabilities,
) {
    let supports_process = if cfg!(any(target_os = "windows", target_os = "macos")) {
        true
    } else if cfg!(target_os = "linux") {
        // Check PipeWire availability at runtime — PulseAudio-only systems
        // don't support per-process capture.
        #[cfg(target_os = "linux")]
        {
            gemacast_core::adapters::capture::pipewire_common::is_pipewire_available()
        }
        #[cfg(not(target_os = "linux"))]
        {
            false
        }
    } else {
        false
    };

    (
        vec![gemacast_core::domain::types::AudioSource::Desktop],
        gemacast_core::domain::types::SenderCapabilities {
            supports_process_capture: supports_process,
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

        let _ = register_device(
            &registry,
            &tray,
            &audio,
            DeviceId("phone-1".into()),
            1,
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

        let _ = register_device(
            &registry,
            &tray,
            &audio,
            DeviceId("phone-1".into()),
            1,
            "My Phone".into(),
            make_addr("192.168.1.2:9000"), // new IP!
            make_addr("192.168.1.2:55559"),
            None,
            None,
            None,
        )
        .await;

        let tray_calls = tray.take_calls();
        // The registry still reports the address replacement to the tray.
        assert_eq!(tray_calls.len(), 2);
        assert!(
            matches!(&tray_calls[0], TrayCall::Lost { device_id, addr } if device_id.0 == "phone-1" && *addr == make_addr("192.168.1.1:9000"))
        );
        assert!(
            matches!(&tray_calls[1], TrayCall::Discovered { device_id, .. } if device_id.0 == "phone-1")
        );

        let audio_calls = audio.take_calls();
        // Registration is transactional: subscribe succeeds before the new
        // registry address is published, so the existing stream is not torn
        // down and recreated during an IP change.
        assert_eq!(audio_calls.len(), 1);
        assert!(matches!(
            &audio_calls[0],
            AudioCall::Subscribe { device_id, .. } if device_id.0 == "phone-1"
        ));
    }

    #[tokio::test]
    async fn register_should_use_none_addr_for_loopback() {
        let registry = MockDeviceRegistry::new();
        let tray = MockTrayNotifier::new();
        let audio = MockAudioController::new();

        let _ = register_device(
            &registry,
            &tray,
            &audio,
            DeviceId("adb-dev".into()),
            1,
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

        let _ = register_device(
            &registry,
            &tray,
            &audio,
            DeviceId("phone-1".into()),
            1,
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

        let _ = unregister_device(
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

        let _ = unregister_device(
            &registry,
            &tray,
            &audio,
            &notifier,
            DeviceId("ghost".into()),
        )
        .await;

        assert!(tray.take_calls().is_empty());
        // Unsubscribe is intentionally idempotent so stale cleanup can race
        // with an already-removed registry entry without leaking a stream.
        assert_eq!(audio.take_calls().len(), 1);
    }

    #[tokio::test]
    async fn approved_lan_request_should_register_and_issue_a_bound_session() {
        let registry = Arc::new(MockDeviceRegistry::new());
        let tray = Arc::new(MockTrayNotifier::new());
        let audio = Arc::new(MockAudioController::new());
        let authorizer = SessionAuthorizer::default();
        let dispatcher = ControlDispatcher {
            registry: registry.clone(),
            tray,
            audio: audio.clone(),
            notifier: Arc::new(MockDeviceNotifier::new()),
            sender_id: DeviceId("pc-1".into()),
            sender_name: "Test PC".into(),
            is_broadcasting: Arc::new(AtomicBool::new(true)),
            authorizer: authorizer.clone(),
        };
        let device_id = DeviceId("phone-1".into());
        let remote_addr = make_addr("192.168.1.5:55559");

        let (pending_tx, pending_rx) = tokio::sync::oneshot::channel();
        dispatcher
            .handle_http_command(ControlCommand::Connect {
                device_id: device_id.clone(),
                device_name: "My Phone".into(),
                source: None,
                remote_addr,
                bitrate: Some(128_000),
                response_tx: pending_tx,
                authorized: false,
                pending_request_id: None,
            })
            .await;
        let pending = pending_rx.await.unwrap().unwrap();
        let request_id = pending
            .pending_request_id
            .expect("the first LAN request must require approval");
        assert_eq!(pending.device_registered, Some(false));
        assert!(!registry.contains("phone-1"));

        for _ in 0..100 {
            if authorizer.pending_status(&request_id, &device_id)
                == Some(gemacast_core::control::PendingApprovalStatus::Approved)
            {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(
            authorizer.pending_status(&request_id, &device_id),
            Some(gemacast_core::control::PendingApprovalStatus::Approved)
        );

        let (approved_tx, approved_rx) = tokio::sync::oneshot::channel();
        dispatcher
            .handle_http_command(ControlCommand::Connect {
                device_id: device_id.clone(),
                device_name: "My Phone".into(),
                source: None,
                remote_addr,
                bitrate: Some(128_000),
                response_tx: approved_tx,
                authorized: false,
                pending_request_id: Some(request_id.clone()),
            })
            .await;
        let approved = approved_rx.await.unwrap().unwrap();
        let token = approved
            .session_token
            .expect("approval must issue a device session token");
        let generation = approved
            .session_generation
            .expect("approval must issue a session generation");

        assert_eq!(approved.device_registered, Some(true));
        assert_eq!(
            approved.pending_request_id.as_deref(),
            Some(request_id.as_str())
        );
        assert!(registry.contains("phone-1"));
        assert!(authorizer.authenticate(&device_id, &token).is_some());
        assert!(authorizer.is_current(&device_id, generation));
        assert_eq!(authorizer.pending_status(&request_id, &device_id), None);
        assert!(matches!(
            audio.take_calls().as_slice(),
            [AudioCall::Subscribe { device_id: subscribed, bitrate: Some(128_000), .. }]
                if subscribed == &device_id
        ));
    }

    mod probe_registration_status {
        use super::*;
        use tokio::sync::oneshot;

        fn make_dispatcher(registry: Arc<dyn DeviceRegistry>) -> ControlDispatcher {
            ControlDispatcher {
                registry,
                tray: Arc::new(MockTrayNotifier::new()),
                audio: Arc::new(MockAudioController::new()),
                notifier: Arc::new(MockDeviceNotifier::new()),
                sender_id: DeviceId("test-sender".into()),
                sender_name: "Test Sender".into(),
                is_broadcasting: Arc::new(AtomicBool::new(true)),
                authorizer: SessionAuthorizer::default(),
            }
        }

        async fn probe(
            dispatcher: &ControlDispatcher,
            device_id: Option<DeviceId>,
        ) -> PresenceResponse {
            let (response_tx, response_rx) = oneshot::channel();
            dispatcher
                .handle_http_command(ControlCommand::Probe {
                    device_id,
                    response_tx,
                })
                .await;
            response_rx.await.expect("probe must answer")
        }

        #[tokio::test]
        async fn the_probe_response_should_report_a_registered_device_as_registered() {
            let registry = Arc::new(MockDeviceRegistry::with_device(
                "phone-1",
                "192.168.1.5:9000",
            ));
            let dispatcher = make_dispatcher(registry);

            let presence = probe(&dispatcher, Some(DeviceId("phone-1".into()))).await;

            assert_eq!(presence.device_registered, Some(true));
            // `is_offline` is a *global* flag and stays independent of this.
            assert!(!presence.is_offline);
        }

        #[tokio::test]
        async fn the_probe_response_should_report_an_evicted_device_as_unregistered() {
            // Empty registry = the watchdog has already evicted us.
            let registry = Arc::new(MockDeviceRegistry::new());
            let dispatcher = make_dispatcher(registry);

            let presence = probe(&dispatcher, Some(DeviceId("phone-1".into()))).await;

            assert_eq!(presence.device_registered, Some(false));
            // The PC process is up; only *our* subscription is gone. If the two
            // were conflated the phone could not tell "resume" from "reconnect".
            assert!(!presence.is_offline);
        }

        #[tokio::test]
        async fn a_probe_without_a_device_id_should_make_no_registration_claim() {
            let registry = Arc::new(MockDeviceRegistry::with_device(
                "phone-1",
                "192.168.1.5:9000",
            ));
            let dispatcher = make_dispatcher(registry);

            let presence = probe(&dispatcher, None).await;

            assert_eq!(presence.device_registered, None);
        }

        #[tokio::test]
        async fn updating_last_seen_on_an_evicted_device_should_not_reregister_it() {
            let registry = MockDeviceRegistry::new();

            // The probe must not resurrect an evicted device — that is what
            // makes the returned status a truthful liveness signal rather than
            // a self-fulfilling one.
            assert!(!registry.update_last_seen(&DeviceId("phone-1".into())));
            assert!(!registry.contains("phone-1"));
            assert!(registry.all_devices().is_empty());

            // …and it must report `true` for one that is still there.
            let live = MockDeviceRegistry::with_device("phone-1", "192.168.1.5:9000");
            assert!(live.update_last_seen(&DeviceId("phone-1".into())));
            assert!(live.contains("phone-1"));
        }
    }
}
