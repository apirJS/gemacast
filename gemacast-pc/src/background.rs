//! Background engine — spawns and wires together all background tasks.
//!
//! Creates the Tokio runtime, constructs all channels, wraps streamers in
//! production adapters ([`crate::adapters`]), and spawns the task set:
//!
//! - **UDP Listener**: Receives presence/probe messages from mobile devices
//! - **Control Dispatcher**: Routes HTTPS and UDP control commands
//! - **Audio Engine**: Captures and streams desktop audio to connected devices
//! - **Command Handler**: Processes tray UI commands (start/stop, kick, shutdown)
//! - **Device Watchdog**: Removes stale devices that stop sending probes
//! - **ADB tasks**: Port forwarding, discovery, and audio tunneling for USB devices
//!
//! ## Construction Phases (Typestate Builder)
//!
//! The background engine is assembled through four compile-time-enforced phases:
//!
//! 1. [`BackgroundEngine::new`] — shared state (`registry`, `is_broadcasting`, `ws_connections`)
//! 2. [`BackgroundEngine::create_channels`] → [`EngineWithChannels`] — all `mpsc`/`broadcast` channels
//! 3. [`EngineWithChannels::create_adapters`] → [`EngineWithAdapters`] — trait object wrappers
//! 4. [`EngineWithAdapters::init_infrastructure`] -> [`EngineReady`] - ADB, UDP, mDNS, HTTPS verified
//!
//! Finally, [`EngineReady::spawn_tasks_and_run`] spawns every background task
//! and awaits completion.

use std::net::SocketAddr;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use tao::event_loop::EventLoopProxy;
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinSet;

use gemacast_core::adapters::capture::DefaultCaptureFactory;
use gemacast_core::adapters::error_notifier::WsErrorNotifier;
use gemacast_core::adapters::process_lister::DefaultProcessLister;
use gemacast_core::control::SessionAuthorizer;
use gemacast_core::control::http::{ControlCommand, ControlServerState};
use gemacast_core::control::messages::ControlMessage;
use gemacast_core::domain::types::DeviceId;
use gemacast_core::network::adb::{
    PresenceProvider, spawn_adb_audio_tcp_server, spawn_adb_discovery_tcp_server,
    spawn_adb_port_forwarding_watchdog,
};
use gemacast_core::stream::streamer::engine::AudioStreamEngine;
use gemacast_core::stream::streamer::engine::StreamSessionFailure;

use crate::adapters::device::WsConnectionMap;
use crate::adapters::{
    ChannelAudioController, EventLoopTrayNotifier, MultiTransportDeviceNotifier,
};
use crate::events::{AppCommand, TrayEvent};
use crate::state::SharedMapDeviceRegistry;
use crate::tasks::{
    audio_engine, command_handler, control_dispatcher, device_watchdog, udp_listener,
};
use crate::traits::DeviceRegistry;
use crate::trusted_devices::TrustedDeviceStore;

// ---------------------------------------------------------------------------
// ADB Presence Provider
// ---------------------------------------------------------------------------

struct PcPresenceProvider {
    is_broadcasting: Arc<AtomicBool>,
    streamer_id: DeviceId,
    streamer_name: String,
}

impl PresenceProvider for PcPresenceProvider {
    fn is_broadcasting(&self) -> bool {
        self.is_broadcasting
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    fn streamer_id(&self) -> DeviceId {
        self.streamer_id.clone()
    }

    fn streamer_name(&self) -> String {
        self.streamer_name.clone()
    }
}

/// Resolve the path to the bundled ADB binary next to our own executable.
///
/// On Windows this is `<exe_dir>/adb.exe`, on other platforms `<exe_dir>/adb`.
/// Falls back to bare `"adb"` (PATH lookup) if the exe directory cannot be determined.
pub(crate) fn local_adb_path() -> std::path::PathBuf {
    let adb_name = if cfg!(target_os = "windows") {
        "adb.exe"
    } else {
        "adb"
    };
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        let local = dir.join(adb_name);
        if local.exists()
            && std::process::Command::new(&local)
                .arg("version")
                .output()
                .is_ok()
        {
            return local;
        }
    }
    #[cfg(target_os = "linux")]
    if std::path::Path::new("/usr/lib/gemacast/adb").exists()
        && std::process::Command::new("/usr/lib/gemacast/adb")
            .arg("version")
            .output()
            .is_ok()
    {
        return std::path::PathBuf::from("/usr/lib/gemacast/adb");
    }
    // Fallback: bare name (will search PATH)
    std::path::PathBuf::from(adb_name)
}

/// Returns a Tokio Command for the bundled ADB.
///
/// On Windows the process is configured with CREATE_NO_WINDOW so no console
/// window flashes when ADB commands run in the background.
#[cfg(target_os = "windows")]
fn adb_command() -> tokio::process::Command {
    let mut std_cmd = std::process::Command::new(local_adb_path());
    use std::os::windows::process::CommandExt;
    std_cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
    tokio::process::Command::from(std_cmd)
}

/// Returns a Tokio Command for the bundled ADB.
#[cfg(not(target_os = "windows"))]
fn adb_command() -> tokio::process::Command {
    let std_cmd = std::process::Command::new(local_adb_path());
    tokio::process::Command::from(std_cmd)
}

// ---------------------------------------------------------------------------
// Background engine entry point
// ---------------------------------------------------------------------------

/// Spawn the background engine on a dedicated thread with its own Tokio runtime.
///
/// Creates all channels, wraps them in production adapters, and spawns
/// every background task into a `JoinSet`.
pub fn spawn_background_engine(
    event_loop_proxy: EventLoopProxy<TrayEvent>,
    command_rx: mpsc::Receiver<AppCommand>,
) {
    std::thread::spawn(move || {
        tracing::info!("Spawning background engine runtime...");
        let rt = match build_tokio_runtime(&event_loop_proxy) {
            Some(rt) => rt,
            None => return,
        };

        rt.block_on(async {
            run_background_tasks(event_loop_proxy, command_rx).await;
        });
    });
}

/// Build a multi-threaded Tokio runtime with max thread priority.
fn build_tokio_runtime(proxy: &EventLoopProxy<TrayEvent>) -> Option<tokio::runtime::Runtime> {
    match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .on_thread_start(|| {
            let _ =
                thread_priority::set_current_thread_priority(thread_priority::ThreadPriority::Max);
        })
        .build()
    {
        Ok(rt) => Some(rt),
        Err(e) => {
            tracing::error!(
                "Fatal error: Failed to build background Tokio runtime: {}",
                e
            );
            let _ = proxy.send_event(TrayEvent::FatalError(e.to_string()));
            None
        }
    }
}

/// The main async body: build the engine through four phases, then run.
async fn run_background_tasks(
    event_loop_proxy: EventLoopProxy<TrayEvent>,
    command_rx: mpsc::Receiver<AppCommand>,
) {
    let result = BackgroundEngine::new(event_loop_proxy)
        .create_channels(command_rx)
        .create_adapters()
        .init_infrastructure()
        .await;

    let Some(engine) = result else { return };

    engine.spawn_tasks_and_run().await;
}

// ---------------------------------------------------------------------------
// Phase 1: Shared state
// ---------------------------------------------------------------------------

/// Phase 1 — holds the core shared state that every subsystem needs.
#[allow(dead_code)] // Fields are consumed by `create_channels()`, not read directly.
struct BackgroundEngine {
    registry: Arc<SharedMapDeviceRegistry>,
    is_broadcasting: Arc<AtomicBool>,
    ws_connections: WsConnectionMap,
    authorizer: SessionAuthorizer,
    trusted_devices: TrustedDeviceStore,
    event_loop_proxy: EventLoopProxy<TrayEvent>,
}

impl BackgroundEngine {
    fn new(event_loop_proxy: EventLoopProxy<TrayEvent>) -> Self {
        Self {
            registry: Arc::new(SharedMapDeviceRegistry::new()),
            is_broadcasting: Arc::new(AtomicBool::new(true)),
            ws_connections: Arc::new(Mutex::new(Default::default())),
            authorizer: SessionAuthorizer::default(),
            trusted_devices: TrustedDeviceStore::load_default(),
            event_loop_proxy,
        }
    }

    /// Create all inter-task channels, consuming `self` and producing the next phase.
    fn create_channels(self, command_rx: mpsc::Receiver<AppCommand>) -> EngineWithChannels {
        let (presence_tx, presence_rx) = mpsc::channel(8);
        let (inbound_control_tx, inbound_control_rx) = mpsc::channel(32);
        let (http_command_tx, http_command_rx) = mpsc::channel::<ControlCommand>(32);
        let (audio_command_tx, audio_command_rx) =
            mpsc::channel::<gemacast_core::stream::streamer::AudioStreamCommand>(32);
        let (adb_shutdown_tx, _) = broadcast::channel::<()>(16);
        let (adb_outbound_control_tx, _) = broadcast::channel::<ControlMessage>(16);
        let (fatal_error_tx, fatal_error_rx) = mpsc::channel::<String>(8);

        EngineWithChannels {
            registry: self.registry,
            is_broadcasting: self.is_broadcasting,
            ws_connections: self.ws_connections,
            authorizer: self.authorizer,
            trusted_devices: self.trusted_devices,
            event_loop_proxy: self.event_loop_proxy,
            command_rx,
            presence_tx,
            presence_rx,
            inbound_control_tx,
            inbound_control_rx,
            http_command_tx,
            http_command_rx,
            audio_command_tx,
            audio_command_rx,
            adb_shutdown_tx,
            adb_outbound_control_tx,
            fatal_error_tx,
            fatal_error_rx,
        }
    }
}

// ---------------------------------------------------------------------------
// Phase 2: Channels created
// ---------------------------------------------------------------------------

/// Phase 2 — all inter-task channels have been created.
struct EngineWithChannels {
    // Shared state (from Phase 1)
    registry: Arc<SharedMapDeviceRegistry>,
    is_broadcasting: Arc<AtomicBool>,
    ws_connections: WsConnectionMap,
    authorizer: SessionAuthorizer,
    trusted_devices: TrustedDeviceStore,
    event_loop_proxy: EventLoopProxy<TrayEvent>,

    // Channels
    command_rx: mpsc::Receiver<AppCommand>,
    presence_tx: mpsc::Sender<(ControlMessage, SocketAddr)>,
    presence_rx: mpsc::Receiver<(ControlMessage, SocketAddr)>,
    inbound_control_tx: mpsc::Sender<(ControlMessage, SocketAddr)>,
    inbound_control_rx: mpsc::Receiver<(ControlMessage, SocketAddr)>,
    http_command_tx: mpsc::Sender<ControlCommand>,
    http_command_rx: mpsc::Receiver<ControlCommand>,
    audio_command_tx: mpsc::Sender<gemacast_core::stream::streamer::AudioStreamCommand>,
    audio_command_rx: mpsc::Receiver<gemacast_core::stream::streamer::AudioStreamCommand>,
    adb_shutdown_tx: broadcast::Sender<()>,
    adb_outbound_control_tx: broadcast::Sender<ControlMessage>,
    fatal_error_tx: mpsc::Sender<String>,
    fatal_error_rx: mpsc::Receiver<String>,
}

impl EngineWithChannels {
    /// Wrap channels in production trait adapters, consuming `self` and
    /// producing the next phase.
    fn create_adapters(self) -> EngineWithAdapters {
        let tray: Arc<dyn crate::traits::TrayNotifier> =
            Arc::new(EventLoopTrayNotifier::new(self.event_loop_proxy.clone()));
        let audio: Arc<dyn crate::traits::AudioController> =
            Arc::new(ChannelAudioController::new(self.audio_command_tx.clone()));
        let notifier: Arc<dyn crate::traits::DeviceNotifier> =
            Arc::new(MultiTransportDeviceNotifier::new(
                self.ws_connections.clone(),
                self.adb_outbound_control_tx.clone(),
                self.adb_shutdown_tx.clone(),
            ));

        EngineWithAdapters {
            registry: self.registry,
            is_broadcasting: self.is_broadcasting,
            ws_connections: self.ws_connections,
            authorizer: self.authorizer,
            trusted_devices: self.trusted_devices,
            event_loop_proxy: self.event_loop_proxy,
            command_rx: self.command_rx,
            presence_tx: self.presence_tx,
            presence_rx: self.presence_rx,
            inbound_control_tx: self.inbound_control_tx,
            inbound_control_rx: self.inbound_control_rx,
            http_command_tx: self.http_command_tx,
            http_command_rx: self.http_command_rx,
            audio_command_tx: self.audio_command_tx,
            audio_command_rx: self.audio_command_rx,
            adb_shutdown_tx: self.adb_shutdown_tx,
            adb_outbound_control_tx: self.adb_outbound_control_tx,
            fatal_error_tx: self.fatal_error_tx,
            fatal_error_rx: self.fatal_error_rx,
            tray,
            audio,
            notifier,
        }
    }
}

// ---------------------------------------------------------------------------
// Phase 3: Adapters created
// ---------------------------------------------------------------------------

/// Phase 3 — trait adapters are ready, infrastructure can now be initialized.
struct EngineWithAdapters {
    // Shared state
    registry: Arc<SharedMapDeviceRegistry>,
    is_broadcasting: Arc<AtomicBool>,
    ws_connections: WsConnectionMap,
    authorizer: SessionAuthorizer,
    trusted_devices: TrustedDeviceStore,
    event_loop_proxy: EventLoopProxy<TrayEvent>,

    // Channels
    command_rx: mpsc::Receiver<AppCommand>,
    presence_tx: mpsc::Sender<(ControlMessage, SocketAddr)>,
    presence_rx: mpsc::Receiver<(ControlMessage, SocketAddr)>,
    inbound_control_tx: mpsc::Sender<(ControlMessage, SocketAddr)>,
    inbound_control_rx: mpsc::Receiver<(ControlMessage, SocketAddr)>,
    http_command_tx: mpsc::Sender<ControlCommand>,
    http_command_rx: mpsc::Receiver<ControlCommand>,
    audio_command_tx: mpsc::Sender<gemacast_core::stream::streamer::AudioStreamCommand>,
    audio_command_rx: mpsc::Receiver<gemacast_core::stream::streamer::AudioStreamCommand>,
    adb_shutdown_tx: broadcast::Sender<()>,
    adb_outbound_control_tx: broadcast::Sender<ControlMessage>,
    fatal_error_tx: mpsc::Sender<String>,
    fatal_error_rx: mpsc::Receiver<String>,

    // Adapters
    tray: Arc<dyn crate::traits::TrayNotifier>,
    audio: Arc<dyn crate::traits::AudioController>,
    notifier: Arc<dyn crate::traits::DeviceNotifier>,
}

impl EngineWithAdapters {
    /// Verify ADB, bind the UDP listener, create HTTPS control state, start
    /// mDNS, and resolve the PC identity. Returns `None` on fatal errors.
    async fn init_infrastructure(self) -> Option<EngineReady> {
        // --- Verify ADB availability ---
        if adb_command().arg("version").output().await.is_err() {
            let msg = "Failed to launch bundled ADB! Please ensure the application was installed correctly.";
            tracing::error!("{}", msg);
            self.tray.notify_fatal_error(msg.to_string());
            return None;
        }

        // --- Identity ---
        let device_name = whoami::devicename().unwrap_or_else(|_| "Desktop PC".to_string());
        let pc_identity = match crate::pc_identity::PcIdentity::load_default() {
            Ok(identity) => identity,
            Err(error) => {
                let msg = format!("Failed to load the PC security identity: {error}");
                tracing::error!("{msg}");
                self.tray.notify_fatal_error(msg);
                return None;
            }
        };
        let streamer_id = pc_identity.device_id();
        let pc_certificate_fingerprint = pc_identity.fingerprint().to_string();
        if let Err(error) = self
            .trusted_devices
            .bind_pc_identity(&pc_certificate_fingerprint)
        {
            let msg = format!("Failed to bind trusted phones to the PC identity: {error}");
            tracing::error!("{msg}");
            self.tray.notify_fatal_error(msg);
            return None;
        }
        let tls_config = match pc_identity.tls_config() {
            Ok(config) => config,
            Err(error) => {
                let msg = format!("Failed to configure encrypted control transport: {error}");
                tracing::error!("{msg}");
                self.tray.notify_fatal_error(msg);
                return None;
            }
        };

        // --- Presence listener ---
        tracing::info!("Initializing UDP Presence Listener...");
        let listener = match gemacast_core::network::PresenceListener::new(self.presence_tx).await {
            Ok(l) => l,
            Err(e) => {
                let msg = friendly_bind_error(e, "Discovery port");
                tracing::error!("Fatal error: {}", msg);
                self.tray.notify_fatal_error(msg);
                return None;
            }
        };

        // On Linux, a default-blocking firewall (firewalld's restrictive zones,
        // or an enabled ufw) silently drops inbound discovery/streaming. The
        // deb/rpm handle this from their maintainer scripts; this best-effort,
        // once-per-session hint covers the AppImage, which has no install hook.
        #[cfg(target_os = "linux")]
        crate::firewall::warn_if_firewall_may_block();

        // --- HTTPS control server state ---
        let control_state = ControlServerState {
            command_tx: self.http_command_tx,
            is_broadcasting: self.is_broadcasting.clone(),
            streamer_id: streamer_id.clone(),
            streamer_name: device_name.clone(),
            ws_connections: self.ws_connections.clone(),
            process_lister: DefaultProcessLister,
            authorizer: self.authorizer.clone(),
            pc_certificate_fingerprint: pc_certificate_fingerprint.clone(),
        };

        // --- mDNS broadcaster ---
        let _mdns_broadcaster = match gemacast_core::discovery::MdnsBroadcaster::new(
            streamer_id.clone(),
            device_name.clone(),
            gemacast_core::network::Ports::CONTROL,
        ) {
            Ok(b) => {
                tracing::info!("Started mDNS broadcaster");
                Some(b)
            }
            Err(e) => {
                tracing::warn!("Failed to start mDNS broadcaster: {}", e);
                None
            }
        };

        // --- ADB presence provider ---
        let presence_provider = Arc::new(PcPresenceProvider {
            is_broadcasting: self.is_broadcasting.clone(),
            streamer_id: streamer_id.clone(),
            streamer_name: device_name.clone(),
        });

        Some(EngineReady {
            registry: self.registry,
            is_broadcasting: self.is_broadcasting,
            ws_connections: self.ws_connections,
            authorizer: self.authorizer,
            trusted_devices: self.trusted_devices,
            event_loop_proxy: self.event_loop_proxy,
            command_rx: self.command_rx,
            presence_rx: self.presence_rx,
            inbound_control_tx: self.inbound_control_tx,
            inbound_control_rx: self.inbound_control_rx,
            http_command_rx: self.http_command_rx,
            audio_command_tx: self.audio_command_tx,
            audio_command_rx: self.audio_command_rx,
            adb_shutdown_tx: self.adb_shutdown_tx,
            adb_outbound_control_tx: self.adb_outbound_control_tx,
            fatal_error_tx: self.fatal_error_tx,
            fatal_error_rx: self.fatal_error_rx,
            tray: self.tray,
            audio: self.audio,
            notifier: self.notifier,
            listener,
            control_state,
            _mdns_broadcaster,
            presence_provider,
            streamer_id,
            device_name,
            pc_certificate_fingerprint,
            tls_config,
        })
    }
}

// ---------------------------------------------------------------------------
// Phase 4: Ready to spawn
// ---------------------------------------------------------------------------

/// Phase 4 — all infrastructure is verified and ready; tasks can be spawned.
struct EngineReady {
    // Shared state
    registry: Arc<SharedMapDeviceRegistry>,
    is_broadcasting: Arc<AtomicBool>,
    ws_connections: WsConnectionMap,
    authorizer: SessionAuthorizer,
    trusted_devices: TrustedDeviceStore,
    #[allow(dead_code)]
    event_loop_proxy: EventLoopProxy<TrayEvent>,

    // Channels (receivers are consumed during spawning)
    command_rx: mpsc::Receiver<AppCommand>,
    presence_rx: mpsc::Receiver<(ControlMessage, SocketAddr)>,
    inbound_control_tx: mpsc::Sender<(ControlMessage, SocketAddr)>,
    inbound_control_rx: mpsc::Receiver<(ControlMessage, SocketAddr)>,
    http_command_rx: mpsc::Receiver<ControlCommand>,
    audio_command_tx: mpsc::Sender<gemacast_core::stream::streamer::AudioStreamCommand>,
    audio_command_rx: mpsc::Receiver<gemacast_core::stream::streamer::AudioStreamCommand>,
    adb_shutdown_tx: broadcast::Sender<()>,
    adb_outbound_control_tx: broadcast::Sender<ControlMessage>,
    fatal_error_tx: mpsc::Sender<String>,
    fatal_error_rx: mpsc::Receiver<String>,

    // Adapters
    tray: Arc<dyn crate::traits::TrayNotifier>,
    audio: Arc<dyn crate::traits::AudioController>,
    notifier: Arc<dyn crate::traits::DeviceNotifier>,

    // Infrastructure
    listener: gemacast_core::network::PresenceListener,
    control_state: ControlServerState<DefaultProcessLister>,
    #[allow(dead_code)]
    _mdns_broadcaster: Option<gemacast_core::discovery::MdnsBroadcaster>,
    presence_provider: Arc<PcPresenceProvider>,
    streamer_id: DeviceId,
    device_name: String,
    pc_certificate_fingerprint: String,
    tls_config: Arc<rustls::ServerConfig>,
}

impl EngineReady {
    /// Spawn every background task and block until all tasks complete.
    async fn spawn_tasks_and_run(self) {
        let mut set = JoinSet::new();

        tracing::info!("Spawning all background tasks...");

        // -- Fatal error relay --
        let tray_for_errors = self.tray.clone();
        let mut fatal_error_rx = self.fatal_error_rx;
        set.spawn(async move {
            while let Some(msg) = fatal_error_rx.recv().await {
                tracing::error!("Fatal background error received: {}", msg);
                tray_for_errors.notify_fatal_error(msg);
            }
        });

        // -- HTTPS control server --
        let (control_shutdown_tx, control_shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        let tray_for_control = self.tray.clone();
        let control_state = self.control_state;
        let tls_config = self.tls_config.clone();
        set.spawn(async move {
            if let Err(e) = gemacast_core::control::start_control_server(
                control_state,
                tls_config,
                control_shutdown_rx,
            )
            .await
            {
                let msg = friendly_bind_error(e, "Control port (55559)");
                tracing::error!("Fatal error: {}", msg);
                tray_for_control.notify_fatal_error(msg);
            }
        });

        // -- UDP listener --
        udp_listener::spawn_udp_listener(
            &mut set,
            self.listener,
            self.presence_rx,
            self.inbound_control_tx.clone(),
            self.tray.clone(),
        );

        // -- Audio engine --
        let error_notifier = WsErrorNotifier::new(self.ws_connections.clone());
        let (session_failure_tx, mut session_failure_rx) =
            tokio::sync::mpsc::unbounded_channel::<StreamSessionFailure>();
        let engine = AudioStreamEngine::new(DefaultCaptureFactory, true, error_notifier)
            .with_session_failure_sender(session_failure_tx);
        audio_engine::spawn_audio_engine(
            &mut set,
            engine,
            self.audio_command_rx,
            self.tray.clone(),
        );

        // Capture/encoder/ADB transport failures have already removed the
        // audio session. Mirror that teardown immediately into the registry,
        // tray, notification transport, and authorization state rather than
        // waiting for the periodic watchdog.
        let failure_registry = self.registry.clone();
        let failure_tray = self.tray.clone();
        let failure_notifier = self.notifier.clone();
        let failure_authorizer = self.authorizer.clone();
        set.spawn(async move {
            while let Some(failure) = session_failure_rx.recv().await {
                let generation = gemacast_core::control::SessionGeneration(failure.generation);
                if !failure_authorizer.is_current(&failure.device_id, generation) {
                    continue;
                }
                if let Some(device) = failure_registry.unregister(&failure.device_id) {
                    failure_tray.notify_device_lost(failure.device_id.clone(), device.addr);
                    failure_notifier
                        .notify_disconnect(&failure.device_id, Some(device.addr))
                        .await;
                }
                failure_authorizer.revoke(&failure.device_id, Some(generation));
            }
        });

        // -- ADB tasks --
        spawn_adb_audio_tcp_server(
            &mut set,
            self.audio_command_tx.clone(),
            self.adb_shutdown_tx.clone(),
            self.fatal_error_tx.clone(),
            self.authorizer.clone(),
        );

        spawn_adb_discovery_tcp_server(
            &mut set,
            self.presence_provider,
            self.inbound_control_tx.clone(),
            self.adb_shutdown_tx.clone(),
            self.adb_outbound_control_tx.clone(),
            self.fatal_error_tx.clone(),
        );

        spawn_adb_port_forwarding_watchdog(&mut set, self.adb_shutdown_tx.clone());

        // -- Device watchdog --
        device_watchdog::spawn_device_watchdog(
            &mut set,
            self.registry.clone(),
            self.tray.clone(),
            self.audio.clone(),
            self.authorizer.clone(),
        );

        // -- Control dispatcher --
        let streamer_id = self.streamer_id;
        let streamer_name = self.device_name;
        let device_auth = crate::device_auth::DeviceAuthManager::default();
        let dispatcher = Arc::new(control_dispatcher::ControlDispatcher {
            registry: self.registry.clone(),
            tray: self.tray.clone(),
            audio: self.audio.clone(),
            notifier: self.notifier.clone(),
            streamer_id: streamer_id.clone(),
            streamer_name: streamer_name.clone(),
            pc_certificate_fingerprint: self.pc_certificate_fingerprint,
            is_broadcasting: self.is_broadcasting.clone(),
            authorizer: self.authorizer.clone(),
            device_auth: device_auth.clone(),
            trusted_devices: self.trusted_devices.clone(),
        });

        control_dispatcher::spawn_control_dispatcher(
            &mut set,
            self.inbound_control_rx,
            self.http_command_rx,
            dispatcher,
            self.registry.clone(),
        );

        // -- Command handler --
        let handler = Arc::new(command_handler::CommandHandler {
            is_broadcasting: self.is_broadcasting,
            streamer_id,
            streamer_name,
            registry: self.registry,
            tray: self.tray.clone(),
            audio: self.audio,
            notifier: self.notifier,
            authorizer: self.authorizer,
            trusted_devices: self.trusted_devices,
            device_auth,
        });

        let (engine_shutdown_tx, mut engine_shutdown_rx) = tokio::sync::oneshot::channel();
        command_handler::spawn_command_handler(
            &mut set,
            self.command_rx,
            handler,
            engine_shutdown_tx,
        );

        // -- Update checker --
        crate::tasks::updater::spawn_update_checker(&mut set, self.tray.clone());

        // --- Wait for shutdown, then stop and join every background task ---
        loop {
            tokio::select! {
                _ = &mut engine_shutdown_rx => break,
                completed = set.join_next() => {
                    match completed {
                        Some(Ok(())) => {}
                        Some(Err(error)) if error.is_cancelled() => {}
                        Some(Err(error)) => tracing::error!("Background task failed: {error}"),
                        None => break,
                    }
                }
            }
        }

        let _ = control_shutdown_tx.send(());
        let _ = self.adb_shutdown_tx.send(());

        let graceful_drain = async {
            while let Some(result) = set.join_next().await {
                if let Err(error) = result
                    && !error.is_cancelled()
                {
                    tracing::error!("Background task failed during shutdown: {error}");
                }
            }
        };
        if tokio::time::timeout(std::time::Duration::from_secs(2), graceful_drain)
            .await
            .is_err()
        {
            set.abort_all();
            while set.join_next().await.is_some() {}
        }

        self.tray.notify_shutdown_complete();

        tracing::info!("Background engine has fully shut down");
    }
}

/// Convert a bind error into a user-friendly message.
fn friendly_bind_error(e: impl std::fmt::Display, port_name: &str) -> String {
    let e_str = e.to_string();
    if e_str.contains("Address already in use")
        || e_str.contains("10048")
        || e_str.contains("98")
        || e_str.contains("WSAEADDRINUSE")
    {
        format!(
            "{port_name} is already in use. Is GemaCast already running in the background? \
             Please check your system tray or Task Manager."
        )
    } else {
        format!("{port_name} failed: {e_str}")
    }
}
