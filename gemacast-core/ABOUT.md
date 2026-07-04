# Gemacast Core Blueprint

## Description
`gemacast-core` is the foundational library for the Gemacast project. It implements the core networking, audio streaming, control logic, and device discovery required to transmit and receive low-latency desktop audio. The library is completely decoupled from UI frameworks, allowing it to be seamlessly embedded in both `gemacast-pc` (Tauri/Desktop UI) and `gemacast-mobile` (Tauri/Android UI).

## Architecture & Patterns

The crate strictly adheres to **Hexagonal Architecture (Ports and Adapters)** to decouple pure business logic from I/O mechanisms like audio hardware interfaces and network sockets.

### Key Patterns Used

1. **Ports and Adapters (Hexagonal)**:
   - **Ports (`src/ports`)**: Define interfaces (traits) for external dependencies (e.g., `CaptureBackend`, `ProcessLister`, `ErrorNotifier`).
   - **Adapters (`src/adapters`)**: Provide the production implementations of these ports (e.g., WASAPI/CPAL for capture, OS-specific process listing).
   - **Domain (`src/domain`)**: Contains pure value objects, domain errors, and core algorithms that have zero I/O dependencies.

2. **Strategy Pattern**:
   - The `CaptureFactory` trait is a strategy pattern allowing the engine to spawn different platform backends (`WasapiDesktopCapture`, `WasapiLoopbackCapture`, `CpalLoopbackCapture`) without changing the core streaming orchestration.

3. **Concurrency via Message Passing (Actor Model)**:
   - Shared mutable state is minimized. Instead, subsystems (like the `AudioStreamEngine` or the `ControlServer`) run isolated `tokio` tasks and communicate strictly via `mpsc` channels and commands (e.g., `AudioStreamCommand`, `ControlCommand`).

4. **Zero-Allocation Data Paths**:
   - The audio hot path (capture -> encode -> network -> decode -> playback) is highly optimized.
   - It utilizes `ringbuf` for lock-free SPSC (Single Producer Single Consumer) queues.
   - The `JitterBufferManager` utilizes pre-allocated vectors (`decode_buf`, `wsola_buf`) to prevent any heap allocations during the high-frequency audio callback loop.

## Component Interaction Diagram

```mermaid
flowchart TD
    subgraph UI App [PC / Mobile Applications]
        UI[App UI/Tray]
    end

    subgraph gemacast-core [gemacast-core Crate]
        direction TB
        
        %% Discovery
        Discovery[Discovery / mDNS]
        
        %% Control
        ControlHttp[Control Server (HTTP/WS)]
        
        %% Audio Pipeline
        subgraph Stream [Stream Engine]
            CapturePool[Capture Pool]
            Encoder[Opus Encoder]
            NetworkTx[UDP/TCP Transport]
        end
        
        subgraph Receiver [Audio Receiver]
            NetworkRx[Packet Listener]
            JitterBuffer[Jitter Buffer & WSOLA]
            Decoder[Opus Decoder / PLC]
        end
        
        %% Ports and Adapters
        Adapters(Platform Adapters: WASAPI / CPAL)
    end

    UI --> ControlHttp
    ControlHttp -- MPSC Commands --> Stream
    CapturePool --> Adapters
    Receiver --> Adapters
```

## Key Workflows

### 1. Control Server & Handshake Flow
- The PC runs the **HTTP Control Server** (`src/control/http.rs`).
- The Mobile device sends an HTTP `POST /connect` with its `DeviceId` and requested `ConnectionMode` (Wifi/USB/ADB).
- The `handle_connect` endpoint dispatches a `ControlCommand::Connect` via an `mpsc` channel to the host application's dispatcher.
- The host application validates the connection and instructs the `AudioStreamEngine` to start streaming to the receiver's IP address.

### 2. Audio Sender Flow
- The `AudioStreamEngine` receives a `Subscribe` command.
- It asks the `CaptureFactory` (Adapter) for a `CaptureHandle` (Desktop or per-process).
- The Platform Capture backend (e.g., WASAPI) begins pushing raw `f32` PCM samples into a lock-free `HeapProd` ring buffer.
- The async `CapturePool` loops, pulling samples from the `HeapCons` ring buffer, encoding them using the `Opus` codec.
- The encoded frames are packed into a `RawPacket` with sequence numbers and sent over the UDP/TCP transport to the subscribed devices.

### 3. Audio Receiver Flow & Jitter Buffer
- The `AudioStreamReceiver` listens on a bound socket and ingests `RawPacket`s.
- Packets are inserted into the `JitterBufferManager` (`src/jitter/manager.rs`).
- The jitter buffer performs advanced latency management:
  - **Dynamic Depth Calculation**: Calculates target buffer depth based on EWMA (Exponential Weighted Moving Average) of network inter-arrival jitter.
  - **Packet Loss Concealment (PLC)**: Instructs the Opus decoder to synthesize audio if packets are missing.
  - **Time-Stretching (WSOLA)**: Uses Waveform Similarity Overlap-Add to seamlessly speed up or slow down audio playback to manage buffer depth without pitch-shifting or popping artifacts.
- Finally, the PCM samples are pulled by the audio playback callback and sent to the DAC. On Android, the app first attempts to build an ultra-low latency Google Oboe stream; if the device's hardware or ROM rejects the stream configuration, it safely falls back to a standard `cpal` stream to guarantee audio delivery.

## File Tree & Explanation

```text
gemacast-core
├── .gitignore
├── ABOUT.md
├── CHANGELOG.md
├── Cargo.toml
└── src
    ├── adapters
     │   ├── capture
     │   │   ├── cpal_loopback.rs
     │   │   ├── mod.rs
     │   │   ├── pipewire_common.rs     (Linux)
     │   │   ├── pipewire_desktop.rs    (Linux)
     │   │   ├── pipewire_process.rs    (Linux)
     │   │   ├── sck_common.rs          (DISABLED — untested)
     │   │   ├── sck_desktop.rs         (DISABLED — untested)
     │   │   ├── sck_process.rs         (DISABLED — untested)
     │   │   ├── wasapi_common.rs
     │   │   ├── wasapi_desktop.rs
     │   │   └── wasapi_loopback.rs
    │   ├── error_notifier.rs
    │   ├── mod.rs
    │   ├── process_lister.rs
    │   └── transport.rs
    ├── audio
    │   ├── mod.rs
    │   └── resampler.rs
    ├── control
    │   ├── http.rs
    │   ├── http_client.rs
    │   ├── messages.rs
    │   ├── mod.rs
    │   ├── types.rs
    │   ├── ws.rs
    │   └── ws_client.rs
    ├── discovery
    │   ├── broadcaster.rs
    │   ├── listener.rs
    │   ├── mdns.rs
    │   └── mod.rs
    ├── domain
    │   ├── error.rs
    │   ├── mod.rs
    │   └── types.rs
    ├── jitter
    │   ├── buffer.rs
    │   ├── manager.rs
    │   ├── mod.rs
    │   └── types.rs
    ├── lib.rs
    ├── network
    │   ├── adb
    │   │   ├── framer.rs
    │   │   ├── mod.rs
    │   │   ├── reverse.rs
    │   │   └── server.rs
    │   ├── interface.rs
    │   ├── mod.rs
    │   └── ports.rs
    ├── ports
    │   ├── capture.rs
    │   ├── error_notifier.rs
    │   ├── mod.rs
    │   ├── process_lister.rs
    │   └── transport.rs
    ├── stream
    │   ├── mod.rs
    │   ├── receiver
    │   │   ├── heartbeat.rs
    │   │   ├── listener.rs
    │   │   ├── mod.rs
    │   │   ├── packet.rs
    │   │   ├── stream.rs
    │   │   └── transport.rs
    │   └── sender
    │       ├── capture_pool.rs
    │       ├── encode.rs
    │       ├── engine.rs
    │       └── mod.rs
    ├── testing.rs
    └── updater
        └── mod.rs
```

### `src/domain/`
Pure logic, zero I/O dependencies.
- `types.rs`: Core value objects (`DeviceId`, `AudioSource`, `JitterConfig`, `TransportType`).
- `error.rs`: Domain-specific error hierarchies (`GemaCastError`, `AudioError`, `NetworkError`).

### `src/ports/`
Hexagonal boundary traits defining what the core needs from the outside world.
- `capture.rs`: Defines `CaptureBackend`, `CaptureHandle`, and `CaptureFactory`.
- `process_lister.rs`: Interface for listing active OS processes (for per-process audio capture).
- `error_notifier.rs`: Abstraction for bubbling up fatal errors to the host application.

### `src/adapters/`
Concrete production implementations of the port traits.
- `capture/`: Contains OS-specific audio capture backends (`wasapi_desktop`, `wasapi_loopback`, `cpal_loopback`, `pipewire_desktop`, `pipewire_process`). ScreenCaptureKit files (`sck_*`) are present but disabled via `#[cfg(FALSE)]` — untested.
- `process_lister.rs`: Implements Windows process listing via `sysinfo`, Linux via PipeWire Registry, macOS returns empty (SCK disabled).
- `error_notifier.rs`: WebSocket-based error notifier.

### `src/control/`
The API surface for device-to-device commands.
- `http.rs`: Axum-based HTTP server handling `/connect`, `/disconnect`, `/change-source`, etc.
- `ws.rs` / `ws_client.rs`: WebSocket implementations for real-time bi-directional control signals.
- `messages.rs`: JSON serialization structures for control commands.

### `src/stream/`
Audio transmission and reception.
- `sender/engine.rs`: The `AudioStreamEngine` orchestrates active subscriptions and dynamically spawns capture tasks.
- `receiver/listener.rs`: Ingests UDP/TCP packets and drives the receiver audio stream.

### `src/jitter/`
High-performance, lock-free audio buffering and manipulation.
- `manager.rs`: The brain of the receiver. Manages dynamic buffer targets, PLC generation, sequence ordering, and WSOLA cross-fading.
- `buffer.rs`: A sequence-ordered storage structure for incoming packets.

### `src/discovery/`
Network discovery protocols.
- `mdns.rs`: Bonjour/mDNS service registration and resolution.
- `broadcaster.rs` / `listener.rs`: UDP broadcast beacons for instantaneous local network discovery.

### `src/network/`
Low-level networking utilities.
- `interface.rs`: IP resolution, local interface detection, and Android USB tethering IP classification.
- `adb/`: Logic specific to ADB reverse port forwarding and multiplexing.
- `ports.rs`: Defines the standard port numbers used by Gemacast services.

### `src/updater/`
Logic for checking and downloading application updates using the latest `updater.json` manifest.
- `mod.rs`: Handles HTTP requests to the release server, parses the update manifest, and streams the downloaded binary.
