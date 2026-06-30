# Gemacast PC Blueprint

## Description
`gemacast-pc` is the GemaCast PC Sender application responsible for capturing desktop audio and streaming it to connected mobile devices. It runs as a system tray application on Windows (with packaging support for Linux and macOS). The application discovers devices via mDNS, UDP broadcasts, and USB (ADB tunneling), manages active connections, and routes audio to subscribed clients.

## Architecture & Flow

The application architecture strictly separates the UI event loop from asynchronous background operations using message-passing channels.

```text
┌─────────────────────────────┐     AppCommand      ┌───────────────────┐
│  Main Thread (tray/UI)      │ ──────────────────► │  Background Engine │
│  app.rs + tray.rs           │ ◄────────────────── │  background.rs     │
└─────────────────────────────┘     TrayEvent       │  └─► tasks/*       │
                                                    └───────────────────┘
```

1. **Main Thread (Tray UI)**: 
   - Managed by `tao` and `tray-icon`.
   - Listens for OS-level events (e.g., shutdown signals) and user interactions from the system tray menu.
   - Receives state updates (`TrayEvent`s) from the background engine to dynamically update the list of connected devices in the menu.

2. **Background Engine**:
   - Runs a multi-threaded Tokio runtime.
   - **Discovery & Presence**: Listens for UDP discovery probes from devices and broadcasts mDNS to announce the PC on the network.
   - **ADB Management**: Manages bundled ADB binaries, handles port forwarding, and facilitates audio tunneling for USB-connected devices.
   - **Control Server**: Handles inbound HTTP control requests (e.g., device connections, disconnections, source/bitrate changes).
   - **Audio Engine**: Captures system audio (using `gemacast_core`) and streams it to all active, subscribed devices.
   - **Device Watchdog**: Periodically evicts stale WiFi devices that stop sending probe heartbeats.

### Key Workflows

- **Startup**: `main()` starts the `tao` event loop (`app::run()`), spawns the background engine on a separate thread, and initializes the tray icon. The background engine spins up its UDP listeners, HTTP control servers, ADB servers, audio engine, and watchdog tasks, wiring them together using channels wrapped in production adapters.
- **Device Connection**: 
  - A device discovers the PC and sends an HTTP `Connect` command.
  - The `ControlDispatcher` handles the command, registers the device in the `DeviceRegistry`, and updates the tray (`TrayEvent::DiscoveredDevice`).
  - It then instructs the `AudioController` to begin streaming audio to the device's IP (or loopback for ADB).
- **Device Disconnection**: 
  - Initiated either explicitly (user clicks "Kick" in the tray -> `AppCommand::KickDevice`, or device sends a `Disconnect` command) or implicitly (watchdog evicts inactive WiFi device).
  - The device is removed from the `DeviceRegistry`.
  - The UI is updated (`TrayEvent::DeviceLost`).
  - The audio subscription is canceled.
  - The device is notified to close its connection via WebSocket or HTTP fallback.

## File Tree & Explanation

```text
gemacast-pc
├── .gitignore
├── ABOUT.md
├── AdbWinApi.dll
├── AdbWinUsbApi.dll
├── CHANGELOG.md
├── Cargo.toml
├── adb.exe
├── build.rs
└── src
    ├── adapters
    │   ├── audio.rs
    │   ├── device.rs
    │   └── tray.rs
    ├── adapters.rs
    ├── app.rs
    ├── background.rs
    ├── events.rs
    ├── main.rs
    ├── state.rs
    ├── tasks
    │   ├── audio_engine.rs
    │   ├── command_handler.rs
    │   ├── control_dispatcher.rs
    │   ├── device_watchdog.rs
    │   ├── udp_listener.rs
    │   └── updater.rs
    ├── tasks.rs
    ├── testing.rs
    ├── traits
    │   ├── audio_controller.rs
    │   ├── device_notifier.rs
    │   ├── device_registry.rs
    │   └── tray_notifier.rs
    ├── traits.rs
    ├── tray.rs
    └── updater.rs
```

### Root Files
- **`Cargo.toml`**: Defines dependencies, workspace settings, and metadata for packaging (`cargo-dist`, `cargo-deb`, `cargo-generate-rpm`). Includes configuration to bundle ADB binaries.
- **`build.rs`**: Build script that embeds a Windows application manifest (fixing a `tray-icon` crash) and automatically downloads and extracts the correct ADB binaries (platform-tools) for the target OS during compilation.

### Source Files (`src/`)
- **`main.rs`**: Application entry point. Initializes tracing and launches the tray event loop.
- **`app.rs`**: Contains the `tao` event loop for the UI. Handles system termination signals, tray menu events, and processes incoming `TrayEvent`s from the background thread.
- **`background.rs`**: The core setup for the background engine. It creates the Tokio runtime, shared state, channels, instantiates production trait adapters, and spawns all async background tasks into a `JoinSet`.
- **`events.rs`**: Defines the inter-thread communication enums: `TrayEvent` (Background -> UI) and `AppCommand` (UI -> Background).
- **`state.rs`**: Implements `SharedMapDeviceRegistry`, a thread-safe registry (`Arc<Mutex<HashMap>>`) of all connected devices.
- **`tray.rs`**: Manages the creation and dynamic updating of the system tray icon and its context menu (adding/removing check-marked devices).
- **`updater.rs`**: Defines platform keys and the application-specific update installation process.
- **`testing.rs`**: Provides hand-written mock implementations of the trait boundaries (`MockTrayNotifier`, `MockAudioController`, etc.) for comprehensive unit testing without actual I/O.
- **`adapters.rs`**: Re-exports the production implementations of traits.
- **`tasks.rs`**: Re-exports all background tasks.
- **`traits.rs`**: Re-exports all trait abstractions.

### `src/adapters/` (Production Implementations)
- **`audio.rs`**: `ChannelAudioController` wraps an `mpsc::Sender` to issue commands to the audio stream engine.
- **`device.rs`**: `MultiTransportDeviceNotifier` notifies devices to disconnect using the best available transport (WebSocket -> ADB loopback -> remote HTTP).
- **`tray.rs`**: `EventLoopTrayNotifier` wraps an `EventLoopProxy` to send events to the tray UI thread.

### `src/tasks/` (Background Async Tasks)
- **`audio_engine.rs`**: Spawns the audio capture and streaming loop (`AudioStreamEngine`), relaying fatal errors to the tray.
- **`command_handler.rs`**: `CommandHandler` task processes `AppCommand`s from the UI, such as starting/stopping broadcasting, kicking devices, and graceful shutdown.
- **`control_dispatcher.rs`**: `ControlDispatcher` processes inbound HTTP and UDP control commands (`Connect`, `Disconnect`, `GetSources`, `ChangeSource`, `ChangeBitrate`, `Probe`). It coordinates the registry, tray, and audio engine.
- **`device_watchdog.rs`**: Periodically scans the `DeviceRegistry` to evict stale WiFi devices that haven't sent a heartbeat within the timeout period.
- **`udp_listener.rs`**: Spawns a listener for UDP discovery probes and relays them to the control dispatcher.
- **`updater.rs`**: Background task that periodically checks for updates and emits `UpdateReady` events.

### `src/traits/` (I/O Abstractions)
These traits decouple business logic from concrete implementations (like channels or HTTP clients), enabling the extensive unit testing found in `testing.rs`.
- **`audio_controller.rs`**: `AudioController` for subscribing/unsubscribing audio streams and changing properties.
- **`device_notifier.rs`**: `DeviceNotifier` for sending disconnect signals to devices.
- **`device_registry.rs`**: `DeviceRegistry` for managing the state of connected devices.
- **`tray_notifier.rs`**: `TrayNotifier` for pushing UI updates to the system tray.
