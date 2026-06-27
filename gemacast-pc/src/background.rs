//! Background engine — spawns and wires together all background tasks.
//!
//! Creates the Tokio runtime, constructs all channels, wraps senders in
//! production adapters ([`crate::adapters`]), and spawns the task set:
//!
//! - **UDP Listener**: Receives presence/probe messages from mobile devices
//! - **Control Dispatcher**: Routes HTTP and UDP control commands
//! - **Audio Engine**: Captures and streams desktop audio to connected devices
//! - **Command Handler**: Processes tray UI commands (start/stop, kick, shutdown)
//! - **Device Watchdog**: Removes stale devices that stop sending probes
//! - **ADB tasks**: Port forwarding, discovery, and audio tunneling for USB devices

use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use tao::event_loop::EventLoopProxy;
use tokio::sync::mpsc;
use tokio::task::JoinSet;

use gemacast_core::adapters::capture::DefaultCaptureFactory;
use gemacast_core::adapters::error_notifier::WsErrorNotifier;
use gemacast_core::adapters::process_lister::DefaultProcessLister;
use gemacast_core::control::http::{ControlCommand, ControlServerState};
use gemacast_core::control::messages::ControlMessage;
use gemacast_core::domain::types::DeviceId;
use gemacast_core::network::adb::{
    PresenceProvider, spawn_adb_audio_tcp_server, spawn_adb_discovery_tcp_server,
    spawn_adb_port_forwarding_watchdog,
};
use gemacast_core::stream::sender::engine::AudioStreamEngine;

use crate::adapters::{
    ChannelAudioController, EventLoopTrayNotifier, MultiTransportDeviceNotifier,
};
use crate::events::{AppCommand, TrayEvent};
use crate::state::SharedMapDeviceRegistry;
use crate::tasks::{
    audio_engine, command_handler, control_dispatcher, device_watchdog, udp_listener,
};

// ---------------------------------------------------------------------------
// ADB Presence Provider
// ---------------------------------------------------------------------------

struct PcPresenceProvider {
    is_broadcasting: Arc<AtomicBool>,
    sender_id: DeviceId,
    sender_name: String,
}

impl PresenceProvider for PcPresenceProvider {
    fn is_broadcasting(&self) -> bool {
        self.is_broadcasting
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    fn sender_id(&self) -> DeviceId {
        self.sender_id.clone()
    }

    fn sender_name(&self) -> String {
        self.sender_name.clone()
    }
}

/// Resolve the path to the bundled ADB binary next to our own executable.
///
/// On Windows this is `<exe_dir>/adb.exe`, on other platforms `<exe_dir>/adb`.
/// Falls back to bare `"adb"` (PATH lookup) if the exe directory cannot be determined.
fn local_adb_path() -> std::path::PathBuf {
    let adb_name = if cfg!(target_os = "windows") {
        "adb.exe"
    } else {
        "adb"
    };
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        let local = dir.join(adb_name);
        if local.exists() {
            return local;
        }
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

/// Gracefully shut down the ADB server that we started, then force-kill
/// any lingering `adb.exe` processes so they don't outlive GemaCast.
async fn shutdown_adb() {
    tracing::info!("Shutting down bundled ADB server...");
    let _ = adb_command().args(["kill-server"]).output().await;
    tracing::info!("ADB server shut down.");

    // Force-kill any remaining adb.exe processes that survived kill-server.
    // This covers edge cases where adb.exe lingers (e.g. stuck fork-server,
    // concurrent adb sessions, or slow daemon teardown).
    #[cfg(target_os = "windows")]
    {
        let mut kill_cmd = std::process::Command::new("taskkill");
        kill_cmd.args(["/F", "/IM", "adb.exe"]);
        use std::os::windows::process::CommandExt;
        kill_cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
        let mut cmd = tokio::process::Command::from(kill_cmd);
        match cmd.output().await {
            Ok(output) if output.status.success() => {
                tracing::info!("Force-killed lingering adb.exe processes");
            }
            _ => {
                tracing::debug!("No lingering adb.exe processes to kill (or already exited)");
            }
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        let mut cmd = tokio::process::Command::new("pkill");
        cmd.args(["-9", "adb"]);
        match cmd.output().await {
            Ok(output) if output.status.success() => {
                tracing::info!("Force-killed lingering adb processes");
            }
            _ => {
                tracing::debug!("No lingering adb processes to kill (or already exited)");
            }
        }
    }
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

/// The main async body: create channels, adapters, and spawn all tasks.
async fn run_background_tasks(
    event_loop_proxy: EventLoopProxy<TrayEvent>,
    command_rx: mpsc::Receiver<AppCommand>,
) {
    let mut set = JoinSet::new();

    // --- Shared state ---
    let registry = Arc::new(SharedMapDeviceRegistry::new());
    let is_broadcasting = Arc::new(AtomicBool::new(true));
    let ws_connections = Arc::new(Mutex::new(HashMap::new()));

    // --- Channels ---
    let (presence_tx, presence_rx) = mpsc::channel(8);
    let (inbound_control_tx, inbound_control_rx) = mpsc::channel(32);
    let (http_command_tx, http_command_rx) = mpsc::channel::<ControlCommand>(32);
    let (audio_command_tx, audio_command_rx) =
        mpsc::channel::<gemacast_core::stream::sender::AudioStreamCommand>(32);
    let (adb_shutdown_tx, _) = tokio::sync::broadcast::channel::<()>(16);
    let (adb_outbound_control_tx, _) = tokio::sync::broadcast::channel::<ControlMessage>(16);
    let (fatal_error_tx, mut fatal_error_rx) = mpsc::channel::<String>(8);

    // --- Adapters (wrap channels in trait implementations) ---
    let tray: Arc<dyn crate::traits::TrayNotifier> =
        Arc::new(EventLoopTrayNotifier::new(event_loop_proxy.clone()));
    let audio: Arc<dyn crate::traits::AudioController> =
        Arc::new(ChannelAudioController::new(audio_command_tx.clone()));
    let notifier: Arc<dyn crate::traits::DeviceNotifier> =
        Arc::new(MultiTransportDeviceNotifier::new(
            ws_connections.clone(),
            adb_outbound_control_tx.clone(),
            adb_shutdown_tx.clone(),
        ));

    // --- Fatal error relay ---
    let tray_for_errors = tray.clone();
    set.spawn(async move {
        while let Some(msg) = fatal_error_rx.recv().await {
            tracing::error!("Fatal background error received: {}", msg);
            tray_for_errors.notify_fatal_error(msg);
        }
    });

    // --- Verify ADB availability ---
    if adb_command().arg("version").output().await.is_err() {
        let msg =
            "Failed to launch bundled ADB! Please ensure the application was installed correctly.";
        tracing::error!("{}", msg);
        tray.notify_fatal_error(msg.to_string());
        return;
    }

    // --- Kill any existing ADB server to get a clean state ---
    let _ = adb_command().arg("kill-server").output().await;

    tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;

    // --- Identity ---
    let device_name = whoami::devicename().unwrap_or_else(|_| "Desktop PC".to_string());
    let sender_id = DeviceId::new();

    // --- Presence listener ---
    tracing::info!("Initializing UDP Presence Listener...");
    let listener = match gemacast_core::network::PresenceListener::new(presence_tx).await {
        Ok(l) => l,
        Err(e) => {
            let msg = friendly_bind_error(e, "Discovery port");
            tracing::error!("Fatal error: {}", msg);
            tray.notify_fatal_error(msg);
            return;
        }
    };

    // --- HTTP control server ---
    let control_state = ControlServerState {
        command_tx: http_command_tx,
        is_broadcasting: is_broadcasting.clone(),
        sender_id: sender_id.clone(),
        sender_name: device_name.clone(),
        ws_connections: ws_connections.clone(),
        process_lister: DefaultProcessLister,
    };

    // --- mDNS broadcaster ---
    let _mdns_broadcaster = match gemacast_core::discovery::MdnsBroadcaster::new(
        sender_id.clone(),
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

    let (_control_shutdown_tx, control_shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let tray_for_control = tray.clone();
    set.spawn(async move {
        if let Err(e) =
            gemacast_core::control::start_control_server(control_state, control_shutdown_rx).await
        {
            let msg = friendly_bind_error(e, "Control port (55559)");
            tracing::error!("Fatal error: {}", msg);
            tray_for_control.notify_fatal_error(msg);
        }
    });

    // --- ADB presence provider ---
    let presence_provider = Arc::new(PcPresenceProvider {
        is_broadcasting: is_broadcasting.clone(),
        sender_id: sender_id.clone(),
        sender_name: device_name.clone(),
    });

    // --- Spawn tasks ---
    tracing::info!("Spawning all background tasks...");
    let error_notifier = WsErrorNotifier::new(ws_connections.clone());
    let engine = AudioStreamEngine::new(DefaultCaptureFactory, true, error_notifier);

    udp_listener::spawn_udp_listener(
        &mut set,
        listener,
        presence_rx,
        inbound_control_tx.clone(),
        tray.clone(),
    );

    audio_engine::spawn_audio_engine(&mut set, engine, audio_command_rx, tray.clone());

    spawn_adb_audio_tcp_server(
        &mut set,
        audio_command_tx.clone(),
        adb_shutdown_tx.clone(),
        fatal_error_tx.clone(),
    );

    spawn_adb_discovery_tcp_server(
        &mut set,
        presence_provider,
        inbound_control_tx.clone(),
        adb_shutdown_tx.clone(),
        adb_outbound_control_tx.clone(),
        fatal_error_tx.clone(),
    );

    spawn_adb_port_forwarding_watchdog(&mut set, adb_shutdown_tx.clone());

    device_watchdog::spawn_device_watchdog(&mut set, registry.clone(), tray.clone(), audio.clone());

    // --- Control dispatcher ---
    let dispatcher = Arc::new(control_dispatcher::ControlDispatcher {
        registry: registry.clone(),
        tray: tray.clone(),
        audio: audio.clone(),
        notifier: notifier.clone(),
        sender_id,
        sender_name: device_name,
        is_broadcasting: is_broadcasting.clone(),
    });

    control_dispatcher::spawn_control_dispatcher(
        &mut set,
        inbound_control_rx,
        http_command_rx,
        dispatcher,
        registry.clone(),
    );

    // --- Command handler (processes AppCommands from tray UI) ---
    let handler = Arc::new(command_handler::CommandHandler {
        is_broadcasting,
        registry,
        tray: tray.clone(),
        audio,
        notifier,
    });

    command_handler::spawn_command_handler(&mut set, command_rx, handler);

    // --- Update checker (runs once at startup, downloads if available) ---
    crate::tasks::updater::spawn_update_checker(&mut set, tray.clone());

    // --- Wait for all tasks ---
    while set.join_next().await.is_some() {}

    // --- Gracefully shut down ADB server so it doesn't linger ---
    shutdown_adb().await;

    tracing::info!("Background engine has fully shut down");
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
