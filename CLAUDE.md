# Gemacast Development Guide

Gemacast streams PC audio to Android over LAN, USB tethering, or ADB. The
workspace uses Rust 2024 and a pinned toolchain from `rust-toolchain.toml`. The
mobile app is Tauri v2 with React 19 and Bun.

## Workspace

| Path | Responsibility |
| --- | --- |
| `gemacast-core` | Shared capture, codecs, stream transport, control protocol, discovery, updater, playback, and jitter logic. |
| `gemacast-pc` | Desktop streamer, tray UI, device registry, and background orchestration. |
| `gemacast-mobile/src-tauri` | Native mobile player, platform services, and Tauri commands. |
| `gemacast-mobile/src` | React UI, controllers, stores, and the Tauri bridge. |

Runtime path:

```text
PC capture -> stereo 48 kHz frames -> Opus/PCM/silence packets
  -> UDP 23556 or ADB TCP 23557
  -> phone receiver -> jitter manager -> Oboe/cpal output
```

Ports are defined only in `gemacast-core/src/network/ports.rs`. The audio wire
format is defined only in `gemacast-core/src/audio/`.

## Commands

Run Rust commands from the repository root:

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo test -p gemacast-core <filter>
cargo run -p gemacast-pc
```

Run frontend commands from `gemacast-mobile/`:

```bash
bun install --frozen-lockfile
bun run format:check
bun run lint
bun run typecheck
bun test
bun run dev
```

Android checks:

```bash
cargo ndk -t arm64-v8a clippy --workspace --exclude gemacast-pc -- -D warnings
cd gemacast-mobile/src-tauri/gen/android
bash ./gradlew :app:testUniversalDebugUnitTest --no-daemon
```

Container checks:

```bash
docker compose -f docker/compose.yaml --profile android up --build --abort-on-container-exit
```

Linux tests require PipeWire and WirePlumber. Android CI uses JDK 17, NDK
25.2.9519653, CMake 3.22.1, build-tools 34.0.0, and platform 36. Shell scripts
and `gradlew` must use LF line endings.

CI validation runs on pull requests to `main` and manual dispatch, not again
after merge. `prepare-release.yml` runs on pushes to `main`; `release.yml` runs
for version tags.

## Architecture boundaries

### Ports and adapters

- Domain and service code depends on traits, not sockets, filesystems, Tauri,
  JNI, or platform APIs.
- Put core ports in `gemacast-core/src/ports/`, PC ports in
  `gemacast-pc/src/traits/`, and mobile ports in
  `gemacast-mobile/src-tauri/src/traits/`.
- Put concrete implementations in the owning crate's `adapters/` directory.
- Wire implementations once at the composition root. Do not construct concrete
  adapters from domain services.
- Platform audio dependencies belong to `gemacast-core`. App crates consume
  core facades rather than importing WASAPI, PipeWire, Oboe, cpal, or
  ScreenCaptureKit dependencies directly.

### PC thread boundary

- Tao's event loop owns the tray and native dialogs and must never block.
- Cross the UI/background boundary only through `TrayEvent` and `AppCommand` in
  `gemacast-pc/src/events.rs`.
- Route all native dialogs through `gemacast-pc/src/dialog.rs`; never call the
  dialog implementation directly from an async task or the Tao event loop.

### Mobile Rust services

- Organize domains under `gemacast-mobile/src-tauri/src/services/`.
- Keep Tauri commands thin: deserialize, call a service, and translate the
  result at the IPC edge.
- Put business behavior on service/domain structs, not in command handlers.
- Platform selection belongs in `services/platform/`. Shared callers use its
  concrete facade; Android and non-Android implementations stay isolated.
- Add a dedicated iOS implementation when behavior diverges. Do not scatter
  target checks throughout shared services.

### React frontend

- Each domain has a controller. Controllers own business actions, workflows,
  side effects, and coordination between stores and the native bridge.
- UI components do not contain business logic.
- Smart components/controllers read stores and connect dependencies. Dumb
  components receive data and callbacks through props.
- One component serves one purpose. Split unrelated rendering or behavior into
  separate components.
- Do not use `useEffect` for derived state, event handling, or business logic.
  Use it only to synchronize an external lifecycle that cannot be expressed as
  an event, controller action, or computed value, and always clean it up.
- `gemacast-mobile/src/core/tauri-bridge.ts` is the only frontend `invoke()`
  surface.
- Tauri event subscriptions belong in `app-events-controller.ts`.
- Stores hold state; they do not perform transport or platform work.
- Keep reusable non-React logic in `core/` and hooks limited to genuine React
  lifecycle or browser behavior.
- Presentational components and controllers must have focused colocated tests.
- Behavior encoded in CSS classes is behavior and needs regression coverage.

## Module refactor rules

### Map before editing

- Identify responsibilities, state, public API, consumers, tests, platform
  branches, wire formats, security checks, retries, and lifecycle rules.
- Keep a pure refactor behavior-preserving. Make any behavior change explicit
  and test it independently.

### Model domains with types

- Give each domain a clearly named struct that owns its state, invariants, and
  behavior.
- Put methods in the file that defines their owning struct. Group files by
  domain struct, not by arbitrary method count.
- Split structs when responsibilities can change independently.
- Use free functions only for stateless transformations or entry points.
- Replace long argument lists and tuples with named request, result, config, or
  handle types.
- Prefer domain names such as `StreamSession` or `UpdateCache`; avoid `Helper`,
  `Manager`, and `Utils` unless the type truly represents that domain.
- Code should explain what it does through types and names. Comments explain
  only reasons, constraints, invariants, unsafe assumptions, or measured tuning.
  Keep comments compact and delete stale narration.

### Files, modules, and visibility

- Keep `mod.rs` as a small facade: private module declarations, deliberate
  re-exports, and minimal wiring.
- Default modules, fields, and helpers to private.
- Consumers import a module's facade, never internal implementation paths.
- Separate serialization models from transport, filesystem, platform, and
  orchestration code.
- Delete obsolete internal APIs after migrating consumers. Do not preserve
  accidental compatibility.

### Platform facades

- Select a platform implementation once at the module boundary.
- Give each platform a concrete backend such as `AndroidPlatform`,
  `NonAndroidPlatform`, or `IosPlatform`.
- Define non-target behavior explicitly as supported, no-op, fallback, or a
  typed unsupported error.
- Keep JNI, Kotlin, OS APIs, and platform dependencies inside the backend.
- Validate affected platforms using their real toolchains; Android Rust checks
  go through `cargo ndk`.

### Errors

- Use `thiserror` for failures callers may distinguish or handle.
- Core errors belong in `gemacast-core/src/domain/error.rs` and join
  `GemaCastError` when crossing a core boundary.
- App-specific errors may live in their owning crate.
- Add variants for actionable categories: invalid input, unavailable data,
  unsupported platform, transport/status, parsing, filesystem, integrity, and
  retry exhaustion.
- Preserve causes with `#[source]` or `#[from]`; attach operation context at the
  layer that knows it.
- Convert to `String` only at UI, IPC, logging, or an existing string-based port.
- Never erase a meaningful error category to shorten propagation.

### Async ownership

- The domain that spawns a task owns its cancellation, replacement, timeout,
  join, and cleanup behavior.
- Return named handles rather than tuples of senders, atomics, and join handles.
- Do not detach work unless detached lifetime is intentional.
- Make reconnect generations and stale-task rejection explicit.

### Consumer migration

- Search the whole workspace for old paths, constructors, functions, fields,
  aliases, serialized names, commands, and events.
- Update composition roots, commands, adapters, desktop, mobile, native code,
  and tests in the same change.
- Replace public struct literals with constructors when fields become internal.
- Search again after migration; every remaining old reference must be
  intentional.

## Security and protocol invariants

- Preserve ports, packet layout, serde names, Tauri command/event names, retry
  behavior, and shutdown order unless a protocol change is explicitly requested.
- Ship protocol-breaking PC and phone changes together.
- Security checks fail closed. Never persist trust earlier, widen
  authentication, or make integrity checks optional.
- Mutating control routes authenticate with `authenticate_device`, binding the
  bearer token to the request `device_id`. Read-only `/sources` and `/processes`
  may use `authenticate_token`.
- Loopback ADB intentionally follows a separate authentication path. Test LAN
  and ADB whenever pairing or session code changes.
- Session tokens are memory-only and rotate on connect. Preserve
  `SessionGeneration` checks so stale requests, sockets, and WebSockets cannot
  remove a newer session.
- Do not reword dispatcher messages used by `connect_error_code` without also
  updating the stable client mapping and tests.
- A PC certificate change remains a hard failure until the user forgets that
  PC. Phone key rotation requires explicit re-pair approval.
- Updater SHA-256 verification is mandatory. Missing or malformed digests fail;
  generated manifests omit unhashable artifacts. The detached GPG signature is
  not verified in-app and must not be described as an in-app trust guarantee.

## Audio and real-time rules

- The audio callback and jitter hot paths must not allocate, block, perform I/O,
  or add burst logging.
- Preserve stereo frame alignment, 48 kHz assumptions, packet sequence behavior,
  and capture/encoder ownership.
- Wi-Fi PCM sends each 10 ms float32 stereo frame as four 978-byte UDP datagrams
  in one batch on the 10 ms frame clock. Reassemble all four before jitter
  playback; preserve sequence and sender generation. A 2.5 ms timer between
  chunks caused constant stutter on a real Windows-to-Android run.
- Keep PCM send age bounded at 40 ms and incomplete receive frames bounded at
  100 ms, including across silence and receive timeouts. A failed chunk makes
  the entire frame incomplete. See `gemacast-core/PCM-TRANSPORT.md` for the wire
  layout and diagnostics.
- Multiple phones are isolated by `DeviceId`; shared sources may share capture,
  but each target owns its encoder and bitrate.
- Treat jitter target calculation and timescale actuation as separate systems.
  Diagnose telemetry before changing tuning constants.
- Every adaptive statistic must decay or age out on its own. Never add an
  unbounded peak latch to target selection.
- Preserve the distinct admission policies for accelerate, preemptive expand,
  and concealment expand. Concealment must not refuse and produce a raw underrun.
- Decoder PLC state is valid only when it describes the last played codec frame;
  reset it on raw, silence, resync, reset, or format changes.
- Crossfade ramps must be monotonic from outgoing to incoming audio and end fully
  on the incoming signal.
- Keep diagnostic counters and audio-path logging. Do not rename or reinterpret
  telemetry without updating analysis and tests.
- Tuning constants require a compact comment naming the measurement or field log
  that justifies the value.
- Unit tests validate logic but cannot validate perceived audio. Jitter,
  latency, concealment, or masking changes require real-device captures across
  the affected transports before being called field-verified.
- Keep one audio behavior change per commit and one independent behavior change
  per measurement round.
- On Android, `tracing` reaches logcat through the `tracing/log` bridge and
  `tauri-plugin-log`. Do not install a separate tracing subscriber.

## Android native layer

- `gemacast-mobile/src-tauri/gen/android/` contains tracked, hand-written Kotlin
  and is not disposable generated output. Never delete or wholesale-regenerate
  it.
- Keep pure Android state transitions in JVM-testable reducers with tests under
  `app/src/test/`.
- Every Kotlin method reached dynamically from Rust/JNI needs an explicit R8
  keep rule in `proguard-rules.pro`.
- Debug JVM tests do not validate R8. Changes to dynamic JNI entry points require
  a release APK check.
- Android build scripts generate required ignored Gradle/Wry artifacts when the
  environment variables in CI are set. Keep package and library names synchronized
  with `tauri.conf.json` and the Rust library target.

## Testing rules

- Keep tests beside the behavior and group them by concern.
- Test names are full behavioral sentences.
- Cover meaningful error variants, fallbacks, platform choices, authorization
  paths, and lifecycle transitions.
- Test observable effects rather than private implementation details.
- Pair rate-limit ceilings with a lower bound or another assertion proving the
  mechanism actually ran.
- Falsify regression tests against the old implementation or each independent
  partial variant; a test that also passes before the fix proves nothing.
- Inject time in tests. Do not rewind `Instant`; advance the test clock from a
  base instant.
- Tests using global daemons must use `serial_test`; PipeWire tests share the
  named `#[serial(pipewire)]` group.
- Mocks live in each crate's `testing.rs` behind `#[cfg(test)]`. Add the owning
  crate's mock whenever adding a port.

Verify changes from narrow to broad:

1. Focused tests.
2. Owning crate or frontend suite.
3. Dependent crates or workspace checks.
4. Formatting and strict linting.
5. Native target checks when platform code changed.
6. `git diff --check`, diff review, and confirmation that unrelated worktree
   changes remain untouched.

## Dependency, release, and configuration rules

- Check the owning `Cargo.toml` before importing a third-party crate.
- Fix actionable RustSec advisories by updating dependencies. Ignore only
  unreachable, upstream-pinned advisories with written reachability evidence.
- Keep `.cargo/audit.toml` and the explicit CI audit ignore list synchronized;
  Docker reads the Cargo audit configuration.
- Do not weaken Tauri CSP. `connect-src` must retain Tauri IPC endpoints;
  development CSP must retain the Vite HMR WebSocket. Verify CSP changes in a
  real app because DOM tests do not enforce CSP.
- macOS 13+ uses ScreenCaptureKit; older/failing systems fall back to cpal and
  may require a virtual device. Screen Recording and Microphone permissions are
  separate. The PC bundle's plist comes from `macos/Info.plist`, not Tauri config.
- macOS releases are currently unsigned and unnotarized; do not imply otherwise
  in installation documentation.
- Versions are managed by release-please. `Cargo.toml` is authoritative;
  release automation synchronizes frontend and Tauri versions.

## Definition of done

A change is complete when ownership is clear, names explain the design, platform
and security boundaries remain explicit, consumers use the intended facade,
obsolete paths are gone, errors remain typed until the edge, behavior is covered,
relevant targets validate through their normal toolchains, and unrelated user
changes remain intact.
