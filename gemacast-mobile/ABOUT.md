# Gemacast Mobile Blueprint

## Description
`gemacast-mobile` is the receiver application for the Gemacast project. It is built using **React** with **Vite** for the frontend, and **Tauri v2** for the backend (Rust), targeting mobile platforms (specifically Android). It allows users to discover `gemacast-pc` senders on the network, establish audio streaming sessions via Wi-Fi, USB, or ADB, and configure receiver settings like jitter buffering, bitrate, and audio sources.

## Architecture & Flow

The application is structured following a strict **Hexagonal Architecture** on the Rust backend and a reactive, hook-driven architecture on the React frontend.

### Component Interaction Diagram

```mermaid
flowchart TD
    %% Frontend
    subgraph Frontend [React Frontend]
        UI[React Components] --> Hooks[Domain Hooks]
        Hooks --> Zustand[Zustand Stores]
        Hooks --> TauriBridge[tauriBridge]
    end

    %% Tauri IPC
    TauriBridge -- IPC Invoke --> TauriCmds[#[tauri::command] Handlers]
    TauriEvents[Tauri Events] -.-> Hooks

    %% Backend
    subgraph Backend [Tauri Backend (Hexagonal)]
        TauriCmds --> AppState[AppState]
        AppState --> AudioService[AudioService]
        
        AudioService --> SessionManager((SessionManager Trait))
        AudioService --> ControlClient((SenderControlClient Trait))
        AudioService --> PlatformService((PlatformService Trait))
        AudioService --> FrontendNotifier((FrontendNotifier Trait))
    end
    
    %% Adapters
    SessionManager -.-> TokioSession[TokioSessionManager]
    ControlClient -.-> HttpClient[HttpSenderControlClient]
    PlatformService -.-> NativePlatform[NativePlatformService / JNI]
    FrontendNotifier -.-> TauriNotifier[TauriFrontendNotifier]
```

### Key Flows

1. **Discovery Flow**:
   - Frontend calls `tauriBridge.startListeningForSenders(mode)`.
   - Backend `domains::discovery` spawns a listener (e.g., UDP multicast) or checks the Android ADB loopback transport.
   - When a sender is found, the backend uses `TauriFrontendNotifier` to emit a `SenderDiscovered` event.
   - Frontend's `useTauriEvents` catches the event, updates `app-store.ts`, and the UI displays the sender in the `SenderList`.

3. **Connection & Audio Streaming Flow**:
   - User clicks a sender -> `connectToSender()` in `use-connection.ts` is invoked.
   - `tauriBridge.connectToSender()` calls the Rust backend.
   - `AudioService` does the following:
     1. Creates an HTTP control client and calls `.connect(...)` to handshake with the PC sender.
     2. Calls `session.start_session(...)` to prepare the audio stream receiver (opening UDP/TCP ports, initializing the jitter buffer, and falling back from Oboe to cpal if necessary).
     3. Calls `platform.set_streaming_flag(true)` and `sync_service(Playing)` to notify the native Android layer (likely updating a foreground service notification).
   - Once connected, `use-connection.ts` fetches available audio sources and capturable processes.
   - It establishes a WebSocket control connection to receive real-time disconnects or errors.
   - The UI optionally requests an OS Wake Lock (via `useWakeLock`) to keep the screen on based on the user's settings.

3. **Disconnection Flow**:
   - User clicks disconnect -> `disconnect()` in `use-connection.ts` is called.
   - Backend `AudioService.disconnect_from_sender()` sends an HTTP disconnect to the PC sender.
   - The session is stopped, and native platform state is updated to `Stopped`.

## File Tree & Explanation

```text
gemacast-mobile
├── .gitignore
├── .prettierignore
├── .prettierrc
├── ABOUT.md
├── README.md
├── bun.lock
├── bunfig.toml
├── eslint.config.js
├── index.html
├── package.json
├── src
│   ├── App.tsx
│   ├── __tests__
│   │   ├── dom-setup.ts
│   │   └── setup.ts
│   ├── assets
│   │   ├── tauri.svg
│   │   ├── typescript.svg
│   │   └── vite.svg
│   ├── components
│   │   ├── device
│   │   │   ├── DeviceInfo.test.tsx
│   │   │   ├── DeviceInfo.tsx
│   │   │   ├── StatusChip.test.tsx
│   │   │   └── StatusChip.tsx
│   │   ├── feedback
│   │   │   ├── Toast.test.tsx
│   │   │   ├── Toast.tsx
│   │   │   ├── ToastContainer.test.tsx
│   │   │   └── ToastContainer.tsx
│   │   ├── latency
│   │   │   ├── LatencyStats.test.tsx
│   │   │   └── LatencyStats.tsx
│   │   ├── layout
│   │   │   └── AppShell.tsx
│   │   ├── senders
│   │   │   ├── EmptyState.tsx
│   │   │   ├── ManualConnect.test.tsx
│   │   │   ├── ManualConnect.tsx
│   │   │   ├── ProcessSelect.test.tsx
│   │   │   ├── ProcessSelect.tsx
│   │   │   ├── SenderCard.test.tsx
│   │   │   ├── SenderCard.tsx
│   │   │   ├── SenderList.test.tsx
│   │   │   └── SenderList.tsx
│   │   ├── settings
│   │   │   ├── BitrateSelect.test.tsx
│   │   │   ├── BitrateSelect.tsx
│   │   │   ├── BufferPresetSelect.test.tsx
│   │   │   ├── BufferPresetSelect.tsx
│   │   │   ├── CustomJitterConfig.test.tsx
│   │   │   ├── CustomJitterConfig.tsx
│   │   │   ├── ExclusiveToggle.test.tsx
│   │   │   ├── ExclusiveToggle.tsx
│   │   │   ├── GainSlider.tsx
│   │   │   ├── ModeSelector.test.tsx
│   │   │   ├── ModeSelector.tsx
│   │   │   ├── NoBufferWarning.tsx
│   │   │   ├── SettingsDrawer.test.tsx
│   │   │   ├── SettingsDrawer.tsx
│   │   │   ├── ThemeToggle.test.tsx
│   │   │   ├── ThemeToggle.tsx
│   │   │   └── UpdateBanner.tsx
│   │   └── shared
│   │       ├── ConfirmDialog.test.tsx
│   │       ├── ConfirmDialog.tsx
│   │       ├── CustomSelect.test.tsx
│   │       ├── CustomSelect.tsx
│   │       ├── HelpDialog.test.tsx
│   │       ├── HelpDialog.tsx
│   │       ├── SegmentedControl.tsx
│   │       └── Toggle.tsx
│   ├── core
│   │   ├── constants.ts
│   │   ├── error.test.ts
│   │   ├── error.ts
│   │   ├── help-content.ts
│   │   ├── latency-tracker.test.ts
│   │   ├── latency-tracker.ts
│   │   ├── persistence.test.ts
│   │   ├── persistence.ts
│   │   ├── presets.test.ts
│   │   ├── presets.ts
│   │   ├── tauri-bridge.test.ts
│   │   ├── tauri-bridge.ts
│   │   ├── types.ts
│   │   ├── validation.test.ts
│   │   └── validation.ts
│   ├── hooks
│   │   ├── use-audio.test.ts
│   │   ├── use-audio.ts
│   │   ├── use-connection.test.ts
│   │   ├── use-connection.ts
│   │   ├── use-custom-preset-editor.test.ts
│   │   ├── use-custom-preset-editor.ts
│   │   ├── use-discovery.test.ts
│   │   ├── use-discovery.ts
│   │   ├── use-drawer.ts
│   │   ├── use-manual-connect.ts
│   │   ├── use-network-monitor.ts
│   │   ├── use-settings.ts
│   │   ├── use-tauri-events.ts
│   │   └── use-updater.ts
│   ├── index.css
│   ├── main.tsx
│   └── stores
│       ├── app-store.test.ts
│       ├── app-store.ts
│       ├── toast-store.test.ts
│       ├── toast-store.ts
│       └── update-store.ts
├── src-tauri
│   ├── .gitignore
│   ├── Cargo.toml
│   ├── build.rs
│   ├── capabilities
│   │   └── default.json
│   ├── icons
│   │   ├── 128x128.png
│   │   ├── 128x128@2x.png
│   │   ├── 32x32.png
│   │   ├── 64x64.png
│   │   ├── Square107x107Logo.png
│   │   ├── Square142x142Logo.png
│   │   ├── Square150x150Logo.png
│   │   ├── Square284x284Logo.png
│   │   ├── Square30x30Logo.png
│   │   ├── Square310x310Logo.png
│   │   ├── Square44x44Logo.png
│   │   ├── Square71x71Logo.png
│   │   ├── Square89x89Logo.png
│   │   ├── StoreLogo.png
│   │   ├── gemacast-pc.png
│   │   ├── gemacast.png
│   │   ├── icon.icns
│   │   ├── icon.ico
│   │   └── icon.png
│   ├── src
│   │   ├── adapters
│   │   │   ├── frontend_notifier.rs
│   │   │   ├── network_info.rs
│   │   │   ├── platform_service.rs
│   │   │   ├── sender_control.rs
│   │   │   └── session_manager.rs
│   │   ├── adapters.rs
│   │   ├── domains
│   │   │   ├── audio
│   │   │   │   ├── commands.rs
│   │   │   │   ├── mod.rs
│   │   │   │   ├── playback.rs
│   │   │   │   └── service.rs
│   │   │   ├── discovery
│   │   │   │   ├── adb_session.rs
│   │   │   │   ├── commands.rs
│   │   │   │   ├── dispatch.rs
│   │   │   │   ├── heartbeat.rs
│   │   │   │   ├── listener.rs
│   │   │   │   ├── mod.rs
│   │   │   │   ├── native.rs
│   │   │   │   ├── probe.rs
│   │   │   │   └── service.rs
│   │   │   ├── ipc
│   │   │   │   ├── mod.rs
│   │   │   │   └── server.rs
│   │   │   ├── updater
│   │   │   │   ├── commands.rs
│   │   │   │   ├── install.rs
│   │   │   │   └── mod.rs
│   │   │   └── mod.rs
│   │   ├── lib.rs
│   │   ├── main.rs
│   │   ├── state.rs
│   │   ├── testing.rs
│   │   ├── traits
│   │   │   ├── frontend_notifier.rs
│   │   │   ├── network_info.rs
│   │   │   ├── platform_service.rs
│   │   │   ├── sender_control.rs
│   │   │   ├── session_manager.rs
│   │   │   └── types.rs
│   │   └── traits.rs
│   ├── tauri.conf.json
│   └── tauri.schema.json
├── tsconfig.json
└── vite.config.ts
```

### Root Configurations
- **`package.json` / `vite.config.ts`**: Standard Vite + React configuration. It includes scripts for linting, testing (`bun test`), and building.
- **`src-tauri/Cargo.toml`**: Defines the Rust dependencies, including Tauri v2, Tokio, and the local workspace crate `gemacast-core`.
- **`src-tauri/tauri.conf.json`**: Tauri application configuration, specifying the bundle identifier (`com.apir.gemacast`) and build commands.

### Frontend (`src/`)

- **`main.tsx` & `App.tsx`**: React entry points. `App.tsx` handles initial device info gathering (UUID, Name, IP) and initializes the global Zustand store before rendering the `AppShell`.
- **`core/`**:
  - `tauri-bridge.ts`: A strictly typed wrapper around Tauri's `invoke` API, bridging the gap between React and Rust commands.
  - `types.ts`, `error.ts`: Domain models and custom error classes (e.g., `GemaCastError`).
  - `persistence.ts`: Handles saving/loading settings and last connected sender via `localStorage`.
- **`stores/`**:
  - `app-store.ts`: The primary Zustand store managing the entire application state (discovered senders, connection status, latency stats, settings).
  - `toast-store.ts`: Manages temporary toast notifications.
  - `update-store.ts`: Manages application update states (available, downloading, installing, ready).
- **`hooks/`**:
  - `use-connection.ts`: Orchestrates the complex logic of connecting, disconnecting, and handling timeouts/errors.
  - `use-audio.ts`: Handles local playback state (starting/stopping the Oboe audio stream without tearing down the connection).
  - `use-discovery.ts`: Triggers the backend sender discovery service.
  - `use-updater.ts`: Coordinates checking for and triggering application updates.
  - `use-wake-lock.ts`: Interfaces with the Web Screen Wake Lock API to prevent the device from sleeping while streaming.
- **`components/`**:
  - Modular UI components organized by feature (`device/`, `senders/`, `layout/`, `settings/`, `feedback/`).
  - `layout/AppShell.tsx`: The main structural layout of the mobile UI.

### Backend (`src-tauri/src/`)

- **`main.rs` & `lib.rs`**: The Rust application composition root. `lib.rs` wires up all the traits to their concrete adapters and registers the `AudioService` inside `AppState`.
- **`state.rs`**: Defines `AppState`, which acts as a simple container holding `Arc<dyn Trait>` abstractions and the `AudioService`.

#### `domains/` (Pure Business Logic)
Contains domain logic that is decoupled from Tauri-specific I/O, making it fully unit-testable.
- **`audio/`**:
  - `service.rs`: `AudioService` coordinates trait dependencies to execute workflows (connect, disconnect, probe, alter bitrate).
  - `commands.rs`: Thin `#[tauri::command]` wrappers that extract state and delegate to `AudioService`.
- **`discovery/`**:
  - `service.rs`: Resolves network identities, IPs, and classifies available transports (Wi-Fi, USB, ADB) by reading from `NetworkInfoProvider` and `PlatformService`.
- **`updater/`**:
  - `commands.rs`: `#[tauri::command]` handlers for checking and downloading updates.
  - `install.rs`: Android-specific logic to trigger APK installation.

#### `traits.rs` (I/O Abstractions)
Defines the boundaries of the Hexagonal architecture.
- `SessionManager`: Manages the audio receiving lifecycle.
- `SenderControlClient`: HTTP client trait for handshaking with `gemacast-pc`.
- `PlatformService`: Interfaces with native Android features (Foreground services, wake locks).
- `FrontendNotifier`: Trait to push events (like discovered senders or latency stats) back to the React UI.

#### `adapters.rs` / `adapters/` (Production Implementations)
Concrete implementations of the traits from `traits.rs`.
- `TauriFrontendNotifier`: Wraps `tauri::AppHandle` to emit IPC events.
- `TokioSessionManager`: Handles Tokio tasks for receiving audio packets and managing jitter buffers.
- `NativePlatformService`: Calls JNI functions to interact with Android native code.
- `HttpSenderControlClient`: Implements the HTTP requests for sender handshakes.

#### `testing.rs` (Unit Testing Mocks)
Provides hand-written mock implementations of all the traits. Mocks (like `MockSessionManager` or `MockPlatformService`) record calls in an internal `Mutex<Vec<Call>>` allowing for thorough, side-effect-free testing of the `domains/` logic.
