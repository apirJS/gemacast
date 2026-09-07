# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

Gemacast streams PC desktop audio to an Android phone over LAN, USB tether, or
`adb reverse`, with a latency budget in tens of milliseconds. Cargo workspace
(edition 2024, Rust pinned to **1.97.1** in `rust-toolchain.toml`), three crates;
the phone app is Tauri v2 + React 19. `README.md` is a one-line stub — this file
is the only prose documentation.

`TEMP/` and `LOGS/` are **gitignored**, so every reference below to
`TEMP/webrtc-neteq` or a `LOGS/log-*.txt` capture is to a *local* working
directory that a fresh clone will not have. Check before following one.

## Commands

### Rust (from repo root)

```bash
cargo fmt --check                                    # CI gate
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace                               # 559 tests, all green
cargo test -p gemacast-core                          # 414 of them; nearly all jitter
cargo test -p gemacast-pc                            # 88
cargo test -p gemacast-mobile                        # 57
cargo test -p gemacast-core rebuffer                 # substring filter
cargo run -p gemacast-pc                             # the PC streamer (tray app)
```

`RUST_LOG` drives the PC subscriber (default `info`); see
[logging.rs](gemacast-pc/src/logging.rs).

On Linux, `cargo test --workspace` needs a live PipeWire **and** WirePlumber
session — CI builds PipeWire 1.2.7 + WirePlumber 0.5.15 from source and runs
under `dbus-run-session`. Read [ci.yml](.github/workflows/ci.yml#L83-L159) before
chasing a Linux-only failure.

### Frontend (from `gemacast-mobile/`, package manager is **bun**)

```bash
bun install --frozen-lockfile
bun run format:check     # prettier
bun run lint             # eslint
bun run typecheck        # tsc --noEmit
bun test                 # 298 tests / 34 files; bun runner + happy-dom,
                         # preloads src/__tests__/{dom-setup,setup}.ts
bun test src/core/presets.test.ts          # single file
bun test --test-name-pattern "auto preset" # single test
bun run dev              # vite only, no native shell
```

**`bun test` is green (298 pass).** Long paired-PC names wrap because both
[ForgetPcIdentity.tsx:81](gemacast-mobile/src/components/settings/ForgetPcIdentity.tsx#L81)
and its sibling [ConfirmDialog.tsx:42](gemacast-mobile/src/components/shared/ConfirmDialog.tsx#L42)
now ship `[overflow-wrap:anywhere]`, not Tailwind's `wrap-break-words`. The
distinction is load-bearing and cost a real overflow bug: `wrap-break-words`
(`overflow-wrap: break-word`) does **not** feed word-break opportunities into an
element's min-content size, so a `flex-1 min-w-0` span holding one unbreakable
token (`PC_<hex>`) refuses to shrink and overflows off-screen; `anywhere` does,
so it wraps. f3048fc fixed `ConfirmDialog` and missed `ForgetPcIdentity`; a later
commit closed the gap. The IDE will suggest the canonical `wrap-anywhere` — do
**not** take it, the test asserts the literal `[overflow-wrap:anywhere]` string.

`format:check` fails on a handful of files for **CRLF line endings** — 4 as of
the Streamer/Player rename (`ForgetPcIdentity.tsx` + its test, `StreamerCard.tsx`,
`use-manual-connect.ts`), and the set churns with checkout state rather than with
anyone's edits. Confirmed cause:
`tr -d '\r' < f | bunx prettier --check --stdin-filepath f` passes on the same
file that fails directly — verified on all 4. Check one that way before believing
it. Do **not** clear it with `prettier --write` — that rewrites line endings
across the tree and buries the real diff. Git normalizes on commit, so the
committed diff stays clean regardless.

### Android

```bash
bun run tauri android build --apk    # from gemacast-mobile/; CI uses bunx
adb logcat                           # gemacast-core `tracing` events land here
cargo ndk -t arm64-v8a clippy --workspace --exclude gemacast-pc -- -D warnings
# from gemacast-mobile/src-tauri/gen/android/ — Kotlin unit tests, a CI gate:
bash ./gradlew :app:testUniversalDebugUnitTest --no-daemon
```

Android clippy is a separate CI gate and must go through `cargo-ndk`. CI pins
**NDK 25.2.9519653**, cmake 3.22.1, build-tools 34.0.0, platforms android-36,
JDK 17 — a local pass on a newer NDK does not guarantee CI. `.cargo/config.toml`
forces `CMAKE_GENERATOR=Ninja` for Android targets, and `audiopus_sys` is
vendored under `vendor/` and patched in via `[patch.crates-io]` because its CMake
build breaks without it. `rust-toolchain.toml` deliberately omits the Android
targets — an Android-only CI step adds them, so the other three jobs do not
download four std libraries they never link.

### CI gates ([ci.yml](.github/workflows/ci.yml))

Six independent jobs; all must pass. `audit` (`rustsec/audit-check`), `frontend`
(bun 1.3.14: format → lint → typecheck → test), and a `backend` matrix of four
legs: **Linux PC**, **Windows PC**, **macOS PC** (fmt + clippy + `cargo test
--workspace`) and **Linux Android** (fmt + `cargo-ndk` clippy + the Gradle JVM
tests, *no* `cargo test`). Releases go through `prepare-release.yml` →
`release.yml` (cargo-dist 0.32.0, `dist-workspace.toml`).

`audit` runs on push/PR only — **deliberately no cron.** A weekly `schedule` was
added and reverted: an advisory disclosed *between* commits is genuinely invisible
to a push-triggered gate (RUSTSEC-2026-0258 (`h2`) landed 2026-08-17 while the
newest run on `main` was 2026-08-10, so the gate never saw it), but a cron is the
wrong instrument for it. GitHub disables scheduled workflows after **60 days of
repository inactivity**, silently, so it stops covering exactly the quiet stretch
it exists for; and with top-level `permissions: contents: read` the action cannot
open an issue, leaving a failure email as the only signal. GitHub's **Dependabot
alerts** read `Cargo.lock` against the same RustSec advisories (mirrored into the
GitHub Advisory Database) and fire when the *database* updates rather than when
you push — continuous, no workflow, not disabled by inactivity. That is where
between-commit coverage belongs. A `cargo audit` finding is fixed with
`cargo update -p <crate>` where semver allows — that is how `h2` and
`event-listener` were cleared.

**Three advisories are ignored, and the ignore list is duplicated by
necessity.** `.cargo/audit.toml` holds it for a local `cargo audit`;
`rustsec/audit-check` does not document reading that file, so ci.yml repeats the
IDs in its `ignore:` input — change one and you must change the other. All three
are pinned by an upstream caret/tilde requirement, so `cargo update --precise`
is rejected outright, and each was checked for reachability before being listed:
`nix` 0.19.1 (RUSTSEC-2021-0119) is in **no shipped target** — absent from both
the `aarch64-linux-android` and Windows trees — and `battery` never calls the
vulnerable `getgrouplist`; `quick-xml` 0.39.4 (RUSTSEC-2026-0194/0195, both 7.5
high) arrives only through `wayland-scanner`, a **proc-macro** that parses
vendored protocol XML at build time and is not linked into any binary, while the
runtime XML parser is already 0.41.0. The reasoning lives in full in
`.cargo/audit.toml` — read it before adding or removing an entry, and never add
one to quiet a *fixable* advisory. The remaining ~20 findings are informational
(`unmaintained` / `unsound` / `yanked`) and do not fail the gate; the GTK3
cluster among them is Tauri's and `tao`'s Linux backend and is not ours to move.

## Architecture

| Crate | Role |
| --- | --- |
| `gemacast-core` | All shared logic: capture, encode, transport, discovery, control protocol, **jitter buffer**, playback. Both apps depend on it. |
| `gemacast-pc` | Streamer. `tao` tray event loop on the main thread; a background thread runs the async engine ([background.rs](gemacast-pc/src/background.rs) → [tasks/](gemacast-pc/src/tasks/)). |
| `gemacast-mobile/src-tauri` | Player. Tauri v2, package name `gemacast-mobile`, lib name `gemacast_mobile_lib`. Composition root is `run()` in [lib.rs](gemacast-mobile/src-tauri/src/lib.rs). |

`gemacast-pc` is split by *thread affinity*, and crossing it is the standard bug.
The `tao` loop owns the tray and every native dialog and must never block;
[events.rs](gemacast-pc/src/events.rs) defines the only two channels across the
line — `TrayEvent` (background → UI) and `AppCommand` (UI → background) — and
[tasks/](gemacast-pc/src/tasks/) holds the six long-lived async tasks
(`audio_engine`, `command_handler`, `control_dispatcher`, `device_watchdog`,
`udp_listener`, `updater`). `control_dispatcher.rs` is where the pairing decision
actually lands and carries the crate's largest test module.

### Runtime data path

```
PC capture (WASAPI / PipeWire / cpal)
  → CaptureResampler → 480-sample stereo frame (10 ms @ 48 kHz)
  → encode (Opus LowDelay/CELT, or raw PCM, or silence marker)
  → UDP :23556  |  TCP :23557 (ADB, length-prefixed via TcpAudioFramer)
  → phone: player/listener → ringbuf → JitterBufferManager
  → Oboe (Android) / cpal audio callback → DAC
```

Wire format: 8-byte sequence number + 1-byte format flag + payload
(`FORMAT_OPUS=0`, `FORMAT_UNCOMPRESSED=1`, `FORMAT_SILENCE=2`), constants in
[audio/mod.rs](gemacast-core/src/audio/mod.rs). Ports are centrally allocated in
[network/ports.rs](gemacast-core/src/network/ports.rs) — discovery UDP 23555,
audio UDP 23556, ADB audio TCP 23557, ADB discovery TCP 23558, control HTTP 23559.
Control is an Axum surface over **TLS** (`/connect`, `/disconnect`, `/sources`,
`/processes`, `/change-source`, `/change-bitrate`, `/probe`, plus `/ws` for push
events) in [control/http.rs](gemacast-core/src/control/http.rs); the phone's half
is [control/http_client.rs](gemacast-core/src/control/http_client.rs). **The audio
ports carry no authentication and no encryption** — only the control channel does.

**The two domain roles were renamed Sender → Streamer and Receiver → Player, and
the wire names went with them, with no compatibility shim.** After the rename,
`Sender`/`Receiver` in this codebase mean exactly one thing: channel endpoints —
so a `Sender` that is not an `mpsc`/`oneshot`/`broadcast`/`watch` endpoint is a
bug. Four names cross a process boundary and were changed deliberately, no
`#[serde(alias)]`, no dual matching, no read-fallback:

| what | old → new | where |
| --- | --- | --- |
| presence field, **snake_case** on UDP 23555 + mDNS | `sender_name` → `streamer_name` | `ControlMessage::Presence` ([messages.rs](gemacast-core/src/control/messages.rs)) — the enum's `rename_all` renames *variants*, not variant fields, hence snake_case; now pinned by a JSON-key assertion in its round-trip test |
| presence field, **camelCase** on `/connect` + `/probe` | `senderName` → `streamerName` | `PresenceResponse` ([types.rs](gemacast-core/src/control/types.rs)) — struct-level `rename_all`, so the same concept ships under two different keys |
| terminal error code, matched by lowercased substring | `sender_offline` → `streamer_offline` | emitted in [http.rs](gemacast-core/src/control/http.rs), listed in `TERMINAL_CONNECT_ERROR_CODES` ([use-connection.ts](gemacast-mobile/src/hooks/use-connection.ts)) |
| localStorage key | `gemacast_last_sender` → `gemacast_last_streamer` | [persistence.ts](gemacast-mobile/src/core/persistence.ts) |

Plus the Tauri IPC surface, all stringly-typed and so runtime-only: 5 commands
(`connect_to_streamer`, `disconnect_from_streamer`, `probe_streamer`,
`start_listening_for_streamers`, `stop_listening_for_streamers`), 3 events
(`streamer-discovered`/`-timeout`/`-connected`) and the `establishWebsocket`
arg `senderIp` → `streamerIp`.

**Consequence: an old PC cannot discover or pair with a new phone, and vice
versa — ship both sides in the same release.** PC and phone self-update
independently ([updater/mod.rs](gemacast-core/src/updater/mod.rs)), so this is
the one failure mode no CI gate can catch. A returning user also loses their
last-connected PC and re-picks it once; the *native* trust store and cert pins
are untouched, so no re-pairing dialog. `streamer-connected` is emitted but
nothing listens to it — dead before the rename, still dead.

### Pairing, device identity and the pinned control channel

Everything under `/…` except `/connect` and `/probe` requires
`Authorization: Bearer <session token>`, and the token is only obtainable by
completing the handshake below. Two different checks, and picking the wrong one is
a silent authorization bug: `authenticate_device` binds the token to the
`device_id` in the body (`/disconnect`, `/change-source`, `/change-bitrate`,
`/ws`), while `authenticate_token` only proves *some* valid session exists — used
for the two read-only routes, `/sources` and `/processes`. This subsystem spans
all three crates plus Kotlin, and it is the most recently churned part of the
repo — read
[control/auth.rs](gemacast-core/src/control/auth.rs),
[control/device_auth.rs](gemacast-core/src/control/device_auth.rs),
[control/tls.rs](gemacast-core/src/control/tls.rs),
[pc/device_auth.rs](gemacast-pc/src/device_auth.rs),
[trusted_devices.rs](gemacast-pc/src/trusted_devices.rs) and
[pc_identity.rs](gemacast-pc/src/pc_identity.rs) together, because no one file
holds the flow.

Five pieces of state decide whether a connection is trusted, each kept somewhere
different — and the last is deliberately *not* durable:

| what | where it lives | lifetime |
| --- | --- | --- |
| PC self-signed P-256 cert + key | files beside `config.json` ([pc_identity.rs](gemacast-pc/src/pc_identity.rs), `rcgen`) | forever; invalid files are *quarantined*, not deleted |
| phone private signing key | **Android Keystore**, non-exportable ([MainActivity.kt:139-153](gemacast-mobile/src-tauri/gen/android/app/src/main/java/com/apir/gemacast/MainActivity.kt#L139)) | forever |
| phone → PC cert pins | app-private `SharedPreferences` | until "Forget PC" |
| PC → phone public keys | `trustedDevices` JSON beside `config.json` | until revoked |
| session token + `SessionGeneration` | **memory only**, both sides | dies with the PC process; rotates every connect |

`/connect` is a **polling multi-round handshake**, not one request — the same
`ConnectReq` is re-POSTed with progressively more fields
([http_client.rs:200-380](gemacast-core/src/control/http_client.rs#L200)):

1. Phone POSTs `device_auth { public_key, phone_nonce }` over a client that
   **pins nothing yet** — deliberate, because an IP-keyed pin strands the client
   when DHCP moves the address to a different PC.
2. PC answers `DeviceAuthChallenge { challenge_id, challenge,
   pc_certificate_fingerprint, pairing_code, requires_approval }`.
3. Both sides independently build the *same* length-prefixed transcript
   (`build_device_auth_transcript`: domain tag ∥ device id ∥ pc id ∥ **cert
   fingerprint** ∥ pubkey ∥ nonce ∥ challenge id ∥ challenge) and derive the
   6-digit `pairing_code` from it. The phone recomputes the code and **rejects a
   mismatch** rather than displaying what the PC sent. The code is not a secret —
   it exists so a human comparing two screens detects a relay.
4. Phone signs the transcript in Keystore, rebuilds its HTTPS client *pinned to
   the observed fingerprint*, and re-POSTs.
5. PC shows a native tray dialog ([app.rs:178](gemacast-pc/src/app.rs#L178) via
   `TrayEvent::ConnectionApproval`); phone shows its own. While the PC user has
   not decided, `/connect` returns **202 Accepted** with `pending_request_id` and
   the phone re-POSTs every 250 ms until `CONNECT_TIMEOUT` (65-70 s).
6. On approval the PC issues the bearer token and persists the phone's public
   key; the phone persists the cert pin **only after** the stream is up, and
   `remember_pc_identity` failing *tears the session back down*.

Consequences worth knowing before touching any of it:

- **The tray dialog must not run on Tao's event-loop thread.** Approval is
  handled by spawning a thread and posting `TrayEvent::ConnectionApprovalResult`
  back with the `oneshot::Sender`, precisely so the modal cannot deadlock the
  loop. All PC dialogs go through [dialog.rs](gemacast-pc/src/dialog.rs) (an
  `rfd` wrapper with a Windows-specific branch) — do not call `rfd` directly.
- **A changed PC cert is a hard failure, not a re-pair prompt** ("forget this PC
  before pairing again"). A changed *phone* key is recoverable, but sets
  `requires_approval`, which forces the code comparison again even for an already
  pinned PC — the post-kick / reinstall path.
- **Loopback ADB connections intentionally omit `device_auth` entirely**
  (see the field comment on `ConnectReq`). Any change here has two paths, and the
  ADB one is the easy one to break silently.
- Verification uses `ring` ECDSA-P256-SHA256, and the fingerprint compare is
  constant-time. `SessionGeneration` exists so a reconnect invalidates delayed
  requests *and* stale WebSockets; `/disconnect` carries the generation for that
  reason. Error strings are mapped to stable codes by string matching in
  `connect_error_code` — **rewording a dispatcher error message silently
  reclassifies it** for the phone UI.
- `TrustedDeviceStore` holds public keys only; challenges and tokens never touch
  disk.

### Discovery and the three transports

`ConnectionMode` / `TransportType` are `Wifi | Usb | Adb`, and the phone picks one
before connecting; `NetworkLink` (7 variants) is the *measured* quality of the
resulting path and is what the jitter profiles key on — the two are not the same
enum and not interchangeable. Three discovery mechanisms coexist in
[discovery/](gemacast-core/src/discovery/): UDP broadcast presence
(`PresenceBroadcaster` / `PresenceListener`, port 23555), mDNS
(`_gemacast._tcp.local.`, instance name = `DeviceId`), and ADB, where the PC
drives `adb reverse` and probes over loopback TCP 23558
([network/adb/](gemacast-core/src/network/adb/)). USB tether needs no separate
discovery — it is Wi-Fi-shaped LAN over RNDIS. The `adb` binary is **bundled next
to the executable** (`local_adb_path()` prefers a sibling `adb`/`adb.exe`, falling
back to `PATH`); the release workflow gates on
`.github/scripts/verify-adb-in-archives.sh`.

### Hexagonal boundaries

All three crates use the same ports-and-adapters shape, and it is load-bearing —
orchestration structs are generic over traits so they unit-test with no hardware:

- `gemacast-core`: [ports/](gemacast-core/src/ports/) (`CaptureFactory`,
  `AudioPacketTransport`, `ErrorNotifier`, `ProcessLister`) ↔
  [adapters/](gemacast-core/src/adapters/); mocks in
  [testing.rs](gemacast-core/src/testing.rs). Static dispatch by default; the
  opt-in `dynamic-dispatch` feature adds `Box<dyn Trait>` wrappers.
- `gemacast-pc`: [traits/](gemacast-pc/src/traits/) ↔ [adapters/](gemacast-pc/src/adapters/).
- `gemacast-mobile`: [traits/](gemacast-mobile/src-tauri/src/traits/) ↔
  [adapters/](gemacast-mobile/src-tauri/src/adapters/) (one file per port), wired
  once in `run()` and handed to services as `Arc<dyn Trait>`.

When adding an I/O dependency, put the trait in the port module and the concrete
type in the adapter module — never reach for the concrete type from a service.

The mobile crate adds a third layer the other two do not have:
[domains/](gemacast-mobile/src-tauri/src/domains/) — `audio`, `discovery`, `ipc`,
`updater`, each `commands.rs` (the `#[tauri::command]` surface, 31 of them
registered in `generate_handler!`) + `service.rs` (the logic, generic over the
`Arc<dyn Trait>` ports). Commands should stay thin: the two files carrying real
behaviour are `domains/audio/service.rs` and `domains/discovery/service.rs`, and
they hold the crate's two largest test modules. `ipc/server.rs` is a **loopback
UDP** listener on an ephemeral port written to `.ipc_port` in the cache dir; it
exists so the Android foreground service can push commands in, and it
deliberately only re-emits them to the frontend — the comment there explains
which race duplicating that logic in Rust caused.

### The Android native layer is hand-written and tracked

`gemacast-mobile/src-tauri/gen/android/` looks like Tauri scratch output but **47
files of it are committed, including 8 Kotlin sources**, and 3 of those are not
generated at all:

- `MainActivity.kt` — the Keystore keypair, ECDSA signing, the trusted-PC
  `SharedPreferences`, the pairing-confirmation dialog, and the JNI entry points
  every `call_native_*` in
  [discovery/native.rs](gemacast-mobile/src-tauri/src/domains/discovery/native.rs)
  reaches. `PlatformService` is the Rust-side port for all of it.
- `PcIdentityConfirmationState.kt`, `PlaybackStateReducer.kt` — pure state
  machines split out *specifically* so they are JVM-unit-testable, with tests in
  `app/src/test/`. That Gradle task is a CI gate.
- `GemaCastService.kt` — the foreground service.

Regenerating this directory, or treating it as disposable build output, destroys
the pairing implementation. Anything here that can be a pure function belongs in
one of the reducer files, next to its test.

**The `:app` module cannot be configured from a fresh clone, and the four missing
pieces come from *build scripts*, not from the Tauri CLI.** Seven artifacts under
`gen/android/` are gitignored; four of them are required just to evaluate the
Gradle build, and `settings.gradle:3` fails outright without the first
(`Could not read script '…/tauri.settings.gradle' as it does not exist`):

| artifact | written by | gated on |
| --- | --- | --- |
| `tauri.settings.gradle`, `app/tauri.build.gradle.kts`, `app/proguard-tauri.pro` | `tauri-build`'s `generate_gradle_files` ([mobile.rs:147](https://docs.rs/tauri-build)) | `TAURI_ANDROID_PROJECT_PATH` |
| `app/src/main/java/com/apir/gemacast/generated/` (10 files incl. `TauriActivity.kt`, `WryActivity.kt`, `proguard-wry.pro`) | `wry`'s and `tauri`'s `build.rs` | `WRY_ANDROID_KOTLIN_FILES_OUT_DIR` (+ `WRY_ANDROID_PACKAGE`, `WRY_ANDROID_LIBRARY`) |

`tauri.settings.gradle` names project dirs by **absolute path into
`~/.cargo/registry`**, so it can never be committed. The remaining three
(`app/tauri.properties`, `app/src/main/assets/`, `app/src/main/jniLibs/`) come
from the CLI and are **not** needed by the JVM unit test — verified by deleting
all seven and running the gate.

Because the gate is env-gated rather than command-gated, `ci.yml` sets those four
variables on the **`Clippy Android` step it already runs** (build scripts execute
under `cargo clippy`), so generation costs no extra compilation and the gate stays
a cheap PR check instead of a duplicate APK build. Deleting all four locally and
re-running that one command reproduced every one of them **byte-identically** to
real CLI output. If you change `identifier` in `tauri.conf.json` or the src-tauri
lib name, update the two literals in ci.yml — a mismatch surfaces as a Kotlin
package/`loadLibrary` compile error, loudly.

**`tauri android init` does not fix this and is not the tool to reach for.**
Measured on a scratch worktree: it leaves every hand-written Kotlin source
untouched (`MainActivity.kt`, both reducers, `GemaCastService.kt`,
`proguard-rules.pro` — so the destruction warning above is about *hand-editing and
wholesale deletion*, not about this command), but it creates none of the four
required artifacts — only two empty directories — **and it silently rewrites a
tracked file**, flipping `BuildTask.kt`'s `val executable` from `bun` to `node`
because it sniffs the package manager from the environment.

**Every JNI entry point needs an explicit R8 keep rule, and nothing in CI would
catch its absence.** Release builds set `isMinifyEnabled = true`, and R8 cannot
see a caller for any of these 15 methods — Rust resolves them by name at runtime
from [services/discovery/native.rs](gemacast-mobile/src-tauri/src/services/discovery/native.rs)
(5 `call_method` sites) and
[services/updater/install.rs](gemacast-mobile/src-tauri/src/services/updater/install.rs)
(1). It is worse than an untraceable call: `call_native_string_method` takes the
Kotlin method name as a **runtime `&str`**, so for most of the 15 there is not
even a literal at the `call_method` site to follow. Two rules *look* like they
cover `MainActivity` and neither does: aapt2 generates
`-keep class com.apir.gemacast.MainActivity { <init>(); }` from the manifest —
**constructor only** — and the generated `proguard-wry.pro` keeps
`com.apir.gemacast.*` but only its `native <methods>`, which these ordinary
Kotlin methods are not. So
[proguard-rules.pro](gemacast-mobile/src-tauri/gen/android/app/proguard-rules.pro)
carries `-keepclassmembers class …MainActivity { public <methods>; }` alongside
the older FileProvider/Intent/Uri block, which exists for exactly the same
reason. `-keepclassmembers` (not `-keep`) is correct because aapt already keeps
the class alive; `public <methods>` covers all 15 without a hand-maintained list
and leaves private helpers minifiable.

The failure mode is **release-only and silent**: the Gradle gate is
`:app:testUniversalDebugUnitTest`, i.e. the *debug*, unminified variant, and
nothing inspects the release dex. Before the rule the methods happened to map
identity while 592 other classes were renamed — R8 declining to act, not a
contract. Verify a change here against a real release APK rather than a stale
`mapping.txt` (one predating the pairing subsystem read as 12 stripped methods
and cost a false alarm):

```bash
bunx tauri android build --apk -t aarch64   # from gemacast-mobile/
unzip -p src-tauri/gen/android/app/build/outputs/apk/universal/release/\
app-universal-release-unsigned.apk 'classes*.dex' | grep -ac signDeviceAuthTranscript
```


The failure mode is **release-only and silent**: the Gradle gate is
`:app:testUniversalDebugUnitTest`, i.e. the *debug*, unminified variant, and
nothing inspects the release dex. Before the rule the methods happened to map
identity while 592 other classes were renamed — R8 declining to act, not a
contract. Verify a change here against a real release APK rather than a stale
`mapping.txt` (one predating the pairing subsystem read as 12 stripped methods
and cost a false alarm):

```bash
bunx tauri android build --apk -t aarch64   # from gemacast-mobile/
unzip -p src-tauri/gen/android/app/build/outputs/apk/universal/release/\
app-universal-release-unsigned.apk 'classes*.dex' | grep -ac signDeviceAuthTranscript
```

### Frontend (React) architecture

Three layers under [gemacast-mobile/src/](gemacast-mobile/src/), and the boundary
between them is enforced by convention only:

- [core/](gemacast-mobile/src/core/) — no React. `tauri-bridge.ts` is the **single
  `invoke()` surface**; nothing else in the app may call `invoke` directly, so
  every command is mockable in one place. Alongside it: `presets.ts` /
  `validation.ts` / `persistence.ts` (the jitter presets, the mirror of
  `JitterConfig::for_link_pair()`), `latency-tracker.ts`, `error.ts`, `types.ts`.
- [stores/](gemacast-mobile/src/stores/) — zustand. `app-store.ts` is one flat
  store holding the whole session (status, discovered streamers, settings, latency,
  link pair, sources); plus `toast-store.ts` and `update-store.ts`.
- [hooks/](gemacast-mobile/src/hooks/) — one hook per concern
  (`use-connection`, `use-discovery`, `use-audio`, `use-network-monitor`,
  `use-tauri-events`, `use-wake-lock`, …). `use-tauri-events.ts` is the only
  place Tauri events are subscribed. Components are presentational and
  co-located with `*.test.tsx`.

Tailwind v4 via `@tailwindcss/vite` (no config file). A behaviour that lives in a
class string still needs a test — see the CRLF/`wrap-break-words` note above for
how that fails.

**The webview is locked down in [tauri.conf.json](gemacast-mobile/src-tauri/tauri.conf.json)
and both settings there have a specific reason.** `withGlobalTauri` is `false` (it
was `true`, which is also not Tauri's own default) — the frontend reaches Tauri
only through `invoke` imported in `tauri-bridge.ts`, so `window.__TAURI__` was an
unused second entry point; grep confirms zero references. `csp` is set rather than
`null`, and the parts that matter:

- `connect-src` must include `ipc: http://ipc.localhost` or **every `invoke` call
  fails** — that is how Tauri v2 carries IPC. It is not injected for you.
- `style-src` needs `'unsafe-inline'` because ~12 components use React `style={{…}}`
  props, which are inline style *attributes*. Nonces cannot apply to attributes, so
  Tauri's build-time nonce/hash injection (which does cover bundled `<script>` and
  `<link>`) does not help here, and per CSP3 a nonce in `style-src` does not void
  `'unsafe-inline'` for the *attribute* type the way it does for `<style>` elements.
- `devCsp` repeats the policy plus `ws://localhost:1420` for Vite HMR. Without a
  `devCsp`, `csp` applies in development too — that is the escape hatch if
  `tauri android dev` breaks.

Nothing in the app loads a remote font, image, or script, so everything else falls
through `default-src 'self'`. **A CSP cannot be verified by any gate in this repo** —
`bun test` runs under happy-dom with no policy, and the bundle is only assembled by
a real `tauri android build`. Verify a change here by running on a device and
watching the webview console, not by a green suite.

### Link-aware buffer configuration

The phone and PC each report a link type and `LinkPair::effective_link()` picks
the *weaker* side ([domain/types.rs](gemacast-core/src/domain/types.rs)). Under
the "Auto" preset (sentinel: `peak_decay_halflife_ms == 0 &&
static_target_ms.is_none()`), `JitterConfig::for_link_pair()` **replaces** the
config wholesale with a per-link profile (ADB/USB, Ethernet, 5 GHz, 2.4 GHz,
unknown). Each profile's numbers carry the field measurement that produced them —
read those before changing a value. Frontend presets live in
[src/core/presets.ts](gemacast-mobile/src/core/presets.ts), validated and
persisted in `validation.ts` / `persistence.ts`.

## The jitter buffer

[gemacast-core/src/jitter/](gemacast-core/src/jitter/) is the most-iterated, most
regression-prone part of the repo (~14k lines including tests, the densest
coverage in the workspace). It runs **entirely on the audio callback thread** — no
allocation, no blocking, no logging bursts in the hot path beyond what is there.

`manager.rs` is the orchestrator; the rest are single-responsibility actors it
drives: `buffer.rs` (packet store), `consts.rs` (tuning constants), `decoder.rs`
(Opus + PLC), `flow.rs` (prebuffer/starvation/concealment state), `stats.rs`
(network observation), `target.rs` (depth control), `timescale.rs` (WSOLA
accelerate/expand/overlap-add/conceal), `types.rs` (shared enums). Its per-callback
flow (`fill_output` → `process_next_frame`) is a dispatcher over named phases
sharing a `FrameContext`; `manager.rs`'s own tests live in `manager_tests.rs`.

**Depth control and actuation are two decoupled layers, and a symptom in one is
routinely misdiagnosed as a bug in the other.** `target.rs` computes *where the
buffer should be* (the `.max()` chain: `min_depth ∨ histogram_base ∨ gap_floor`);
`manager.rs` + `timescale.rs` *physically move it there*. A correct target with an
inert actuator looks exactly like a target that is too high — the buffer parks
above the band, and the obvious-looking fix is to lower the band, which makes it
worse. v13 lowered the band while the actuator was dead; v14 repaired the actuator
first and only then raised the target back. **Read the per-window `accel` /
`expand` / `declined_*` counts on the 1 Hz depth line before concluding the target
is wrong.**

The depth controller reached its designed shape in v19 and **is closed**:
`winning_term = gap_floor` in 514/514 and 430/430 windows across both v19
captures, zero off-formula windows, `target >= max_gap` in 45/46 and 12/12. It is
effectively single-term — NetEQ's shape — so a depth symptom now points at the
actuator, the resume path, or the link, not at the `.max()` chain. v21 re-priced
the two obvious depth fixes for starvation and **rejected both with numbers**: a
25% sweep of the `gap_floor` multiplier does not move uncompressed coverage *at
all* (7/19 throughout) because the gap **steps up** by mean 10.4 frames at the
starving window — a margin sized for a step change *is* that latency; and gap-window
memory tops out at 9/12 and 6/12 covered even with *perfect* two-minute memory, for
+99/+123 ms on every window. Growth cannot bridge it either (1 splice per 200 ms,
~5.3% of the drain rate). Do not relitigate these without new data.

**Grade the link before grading the round.** Read in isolation, v21's 2.4 GHz numbers
look like a regression (concealment share 0.583 → 1.031%, starvation 2.69 → 4.09
ev/min). They are not: all three v21 commits are downstream of the decision to
conceal and cannot move starvation frequency in either direction, and the link was
measurably worse — `max_gap` median 21.0 → 21.8 and 16.2 → **30.4**, max 34.0 → 54.2
and 36.5 → 53.0, `window_ms > 1500` **0/438 → 23/554** and **0/507 → 12/456**,
burst-detected windows 26 → 58%. Mann-Whitney U on the `max_gap` distributions:
**z = −7.72, p = 1.15e−14** (unc) and **z = −15.56, p = 1.46e−54** (128k). The
concealment share rose because there was more to conceal. Compare `max_gap`, the
stall count and the burst share across captures *first*, every round, before
attributing a delta to a commit.

**USB cannot reach a 10–20 ms target, and the reason is the link's delivery grid.**
Asked directly in v21 and answered from 335 Auto windows: `min_depth` 3 → 0 frames
buys **0.8 ms** (mean raw target 3.45 → 3.37), because the binding term is `gap_floor`
and it is honest — USB `max_gap` is mean 2.37 / median 2.2 / p90 2.9 frames (the
USB-transit batching the profile comment already describes), burst detection fires in
**0%** of windows, and **`gap_floor ≤ 2.0` in 0 of 335 windows**. USB delivers in
~22 ms batches; a 10–20 ms adaptive target would be a target below the arrival period.
The fixed-buffer Wired preset is the only way to ask for one, and it asks by
forgoing cover rather than by measuring less.

Invariants earned in field tests:

- **No statistic may latch its own history.** Every depth signal must fall on its
  own, by ageing out of a window or by recomputation from a bounded history.
  Peak-latches (`ema_peak`, `max_iat_cumulative_sum`) were removed after pinning
  the target at the comfort cap while the honest gap signal read 10 frames; v21
  found the same shape in the *concealment fade*, keyed to a counter that froze
  (below). The same warning is repeated in `compute_target_depth` — do not add a
  term to the `.max()` chain that cannot fall on its own.
- Several mechanisms are deliberate ports of WebRTC NetEQ (relative-arrival-delay
  histogram, `BufferLevelFilter`, `BufferLimits`, timescale cooldowns,
  `consecutive_expands_`). Upstream is checked out at `TEMP/webrtc-neteq` —
  consult it before inventing a heuristic.
- **The RMS thresholds no longer gate whether a splice is attempted** —
  `SILENCE_RMS`, `ARTIFACT_MASK_RMS` in
  [jitter/consts.rs](gemacast-core/src/jitter/consts.rs). Removing psychoacoustic
  masking wholesale was once the worst regression here, and that warning still
  stands for `SILENCE_RMS` — live on the silence fast-forward shed, free silence
  growth, and the NCC gate's VAD escape. `ARTIFACT_MASK_RMS` was demoted to a
  logged observation in v14 (`tally.loud_splices`, `mask_rms=`) after declining
  **93-96% of 36,455 drain attempts across 881 s on every link**, leaving 243
  splices — the drain had no actuator on program material at all. NCC is the
  quality gate now; `declined_rms_mask` is still counted precisely so a non-zero
  reading says the gate came back by a path nobody intended.
- **Our VAD escape is inert, so an NCC threshold acts as a hard AND.** Upstream's
  `active_speech` is *relative to measured background noise*
  ([time_stretch.cc:174-200](TEMP/webrtc-neteq/time_stretch.cc#L174)); ours is
  `rms >= SILENCE_RMS`, an absolute floor music clears continuously. Price any NCC
  change as though the escape does not exist, because it does not.
- **Three time-stretch operations, three admission rules.** v20 found we had
  merged the last two and applied the strictest rule to the one upstream leaves
  open:

  | ours | upstream | admission |
  | --- | --- | --- |
  | `accelerate` | `Accelerate` | NCC 0.9 `\|\| !active_speech` |
  | `expand` | `PreemptiveExpand` | `EXPAND_NCC_THRESHOLD` 0.85 `\|\| !active_speech` |
  | `expand_conceal` | `Expand` | **none — concealment never refuses** |

  At `occupied <= 1` the splice replaces *raw underrun*, so a badly-correlated
  splice beats the silence it displaces:
  [expand.cc:438-455](TEMP/webrtc-neteq/expand.cc#L438) picks a winner by a
  correlation/distortion ratio and always emits. `declined_underrun_ncc` must read
  **0** in every capture — same tripwire role as `declined_rms_mask`. Growth's
  0.85 is calibrated, not rounded: mixed-material acceptance **2.0% at 0.90 vs
  59.7% at 0.85**, knee at 0.80 (−5.72 dB dip), and the field confirmed it —
  preemptive acceptance **11.9 → 41.0%** (unc) and **7.1 → 40.4%** (128k) from v19
  to v20, with uncompressed starvation halving (5.21 → 2.70 ev/min, RR 0.518,
  p=0.013). The drain keeps its own 0.9.
- **The rebuffer resume clamp reads the band at `max(target, raw_target)`, and the
  `.max()` is load-bearing, not cosmetic.** `target` is the *ramped* value from
  `control.advance()` and lags the measurement; `raw_target` is the live,
  comfort-capped answer. v19 clamped on `target` alone and discarded cover the link
  had just proved it needed (below `max_gap` on 6.1% of uncompressed resumes;
  shortfall 11.75 → 1.74 frames and discard 660/850 → 110/20 ms on 128k; margin
  causality Spearman r=+0.570, p=0.0005, n=33, with 73.9% of uncompressed
  starvations within 10 lines of a resume). But `raw < target` on 11/33 resumes
  during descent, so `buffer_limits(raw_target)` *alone* would clamp **lower** than
  v19 on a third of them. `buffer_limits(t).high` is monotone non-decreasing in `t`
  (pinned by `the_high_limit_must_never_fall_as_the_target_rises`), so the max makes
  the change provably one-directional. Cost: above `high_limit` on 12/33 and 8/8,
  mean 4.58/10.38 frames, max 21, shed in ~630 ms, never past the 80-frame cap.
  **This is not the v7 change** — v7 wired `max_gap` into the *release threshold*
  and 5 GHz jumped 3 → 21 frames. `unpause_threshold` stays a pure function of
  `target`; the clamp only decides how much of an already-released burst is kept.

### Concealment — what plays when nothing arrives

Two rounds, two halves of the same defect. v21 fixed **what** is emitted, after the
v20 captures showed the generator producing **exact digital silence** — all 267
concealed frames / 2674 ms (0.583%) of the uncompressed run measured
`rms=0.00000000`. v22 fixed **the gain schedule applied to it**, which was written
for the output v21 replaced and was still muting six holds in ten to zero. The v21
half is field-confirmed on all five links: **zero** fallbacks to the silent codec
path, 565 concealed frames on 2.4 GHz alone, and every tripwire
(`declined_rms_mask`, `declined_underrun_ncc`, `splice_step > 1.0`) still reading 0.
Five coupled facts, all live:

- **`decode_plc()` on a decoder that was never fed returns zeros.** Measured, not
  assumed: a virgin decoder peaks at exactly `0.0` on all ten PLC frames, while a
  warmed one decays 0.2115 → 0.0182. `capture` never touches the codec on the
  uncompressed or silence paths, so *every* concealment there ran on that state.
  `FrameDecoder::plc_ready()` is the gate — backed by a private `codec_state:
  CodecState` of `Warm`/`Cold` — and it means "the codec's state
  describes the frame we most recently played" — **not** "has this decoder ever
  seen Opus". A mid-session `/change-bitrate` leaves a fed decoder whose state
  drifts one frame further from the truth per frame; extrapolating from that is
  worse than repeating audio that actually played. Set `Warm` in `decode_opus` and
  `decode_plc`; set `Cold` in `new`, the silence branch, the uncompressed branch
  (*after* the sub-branches, so the empty-payload fallback is covered too),
  `resync`, and `reset`.
- **`TimeScaler::conceal_frame` repeats the last played pitch period** when the
  gate is down, and its geometry is exact rather than approximately right.
  `n = 480`, `anchor = n - OLA_LEN = 352`, `search_limit = min(SEARCH_RANGE,
  anchor - OLA_LEN) = 224`, so `best_d ∈ [0, 223]` and `P = anchor - best_d ∈
  [129, 352]` (136–372 Hz). It emits the verbatim run `hist[n-P..anchor]` then an
  `OLA_LEN` crossfade; the crossfade ends at `w = 1` on `hist[n-P-1]` and the next
  period opens on `hist[n-P]` — **source-adjacent at every P, so the wrap step is
  the material's own step**. The emitted frame goes into `hist_buf` *un-muted* and
  the caller fades only its own copy, so gain never compounds and consecutive
  concealed callbacks stay in phase (NetEQ appends its expansion to
  `sync_buffer_` for the same reason). `expand_conceal` cannot cover this case —
  with history only, `expand_inner` sees `anchor 352 < hist_frames 480` and
  refuses. Both fallbacks (`plc_ready()`, or no history staged) land on the codec
  path unchanged, so **the Opus path is bit-identical by construction**.
- **The fade is keyed to `flow.conceal_run`, not `starvation_count`.** The latter
  freezes at `REBUFFER_AFTER` for a whole rebuffer hold — the hold conceals from
  an early return that never increments it — which pinned the gain at a constant
  `1.0 - (5-3)/4 = 0.5` for every callback of every hold: 158 ms of flat-gain
  repetition, the metallic failure mode. `conceal_run` is incremented by the
  generator itself, so it spans both paths and falls to zero on the first real
  frame.
- **The fade must not reach zero, and v22 is where it stopped doing so.** The
  schedule v21 inherited (`1.0 - (conceal_run - 3)/4`, floored at 0.0) hit exact
  digital silence at `conceal_run == 7` — 60 ms into any run. Across the 64
  rebuffer holds of the five-link v21 round that muted **382 frames / 3820 ms** to
  zero: 57.9% of every 2.4 GHz hold, 51.7% of 128k, 77.5% of ADB, with **39/64
  (60.9%)** of holds running past frame 7 (mean 10.8 frames, median 10, p95 26).
  That is the "dropout" the field reported, and it was the only symptom v21 left
  standing. The schedule was *correct when written* — it faded `decode_plc()`
  output, where 60 ms of extrapolation genuinely does sound worse than silence. v21
  changed what is being faded: on every uncompressed link the concealed frame is
  now `conceal_frame` output, audio that really played, replayed. v22 slows the
  slope to `/12` and floors it at `CONCEAL_FADE_FLOOR` = 0.15 (−16.5 dBFS,
  [consts.rs](gemacast-core/src/jitter/consts.rs)), reaching the floor at
  `conceal_run == 14` (140 ms). The `> 3` threshold is unchanged. Both halves are
  load-bearing: a floor alone removes every zero but reaches bottom at 70 ms and
  sits there (mean gain 0.414/0.461); the slope alone still zeroes 18.1%/13.9% of
  hold frames. Together, **zero silent frames on every link**, mean gain 0.327 →
  0.576 and 0.384 → 0.633. 0.15 also sits well above `SILENCE_RMS` (0.005), so
  neither the silence fast-forward shed nor the NCC gate's VAD escape changes state
  because of this gain.
- **Upstream does not fade to silence either, and its schedule is commonly
  misread** (this file said otherwise before v22). `mute_slope` is *signal-derived*
  ([expand.cc:730-763](TEMP/webrtc-neteq/expand.cc#L730)) and is set to literally
  **0** — no muting at all — for strongly-voiced material (`slope > 8028`). The
  fixed constants at [expand.cc:257-266](TEMP/webrtc-neteq/expand.cc#L257) are
  gentle floors applied at `consecutive_expands_` 3 and 7 (1.0 → 0.95, 1.0 → 0.90
  over 6.25 ms each), and `kMaxConsecutiveExpands = 200` — **two seconds** — before
  a run is called excessive. When `mute_factor` does reach 0 the output is not
  silence: [expand.cc:295](TEMP/webrtc-neteq/expand.cc#L295) hands over to
  `GenerateBackgroundNoise`. There is no 120 ms zero-terminus to port.

**The risk v22 accepts, and the counter that prices it.** A long run now repeats one
pitch period at the floor gain for its whole tail, where upstream rotates three lags
([expand.cc:844-853](TEMP/webrtc-neteq/expand.cc#L844)) so consecutive expansions are
never identical. `conceal_frame` re-stages its history from its own output, so the
pitch search reconverges on the same period — the stuck-note failure mode the old
fade avoided by truncating to silence. Bounded three ways: `max_missing_for` resets
the stream at 100 frames (ADB/USB/5 GHz) or 300 (2.4 GHz), the measured p95 hold is
26 frames, and the floor gain is low. `tally.floor_frames` (`floor_frames=` on the
1 Hz depth line) counts frames emitted *at* the floor, which `conceal_run_max`
cannot separate — one 40-frame run and four 14-frame ones read identically there and
26:4 here. **Lag rotation is the deferred follow-up**, gated on whether a capture
shows a floor tail long enough to matter.

### Splice geometry

All four splice paths (`overlap_add`, `expand`, `expand_conceal`, `accelerate` —
three crossfade sites, since both expands share `expand_inner`) fade over
`OLA_LEN` = 128 sample-frames using `fade_ramp`, a **monotonic** raised-cosine
ramp — exactly 0 at `i=0`, exactly 1 at `i=OLA_LEN-1`. Monotonicity is the
load-bearing property, not the shape. Through v17 this was a full Hann *bell*
(0 → 1 → 0), so `fade_in` returned to ~0.0006 at the end and the crossfade closed
on the **outgoing** signal while the verbatim tail resumed from the **incoming**
one. Terminal step over the signal's own max slope: bell 0.37–6.6×, ramp ≤1.00× —
1.00× being the floor, since the last faded sample lands on the signal's own next
sample, so the "step" *is* the signal's step. Fixed in v18 and confirmed in the
field every round since — v19 `splice_step` n=204, mean 0.520, p95 0.910, **max
1.000 with zero readings above** (the bell would have read 19.28), and still
**0 readings above 1.00** across all five v21 links. That counter is the standing
tripwire for any change here. Three traps:

- **A steady tone cannot detect a fade-shape bug.** At an exact pitch multiple
  `early[i] == late[i]`, so any weights summing to 1 give byte-identical output —
  the bell survived four rounds of green tests that way.
  `timescale::tests::crossfade_ramp` uses a *gain-tapered* tone instead: NCC is
  gain-invariant so the gate still passes, but the aligned sections hold different
  values. Verify a new fade test fails against the bell before trusting it. The
  end **phase** matters as much as the taper: with the last sample on a zero
  crossing, a naive whole-frame repeat and a correct source-adjacent join differ by
  only ~1.5× (measured 0.00496 vs 0.00339) and no assertion can separate them.
  `pitch_concealment`'s helper anchors that sample at `π/4`, where the ratio is
  ≈25×.
- **Fade length is not splice offset.** NetEQ's fade length *is* the pitch period
  (`output->CrossFade(temp_vector, peak_index)`,
  [accelerate.cc:81](TEMP/webrtc-neteq/accelerate.cc#L81)); its 15 ms is where the
  splice *begins*, so comparing our 2.67 ms `OLA_LEN` against 15 ms is a category
  error. A period-sized variable fade was modelled in v18 and measured **identical
  to the fixed 128 in every row**; the correlation reference is also glued to the
  window tail (`anchor = n - OLA_LEN`), so a longer fade is a no-op unless the
  anchor moves back with it. Deferred with numbers against it.
- **`splice_step` and `conceal_step` are separate fields on purpose.** The
  concealment *entry* is a hard join, not a crossfade, so it legitimately reads
  above 1.00; folding it into `splice_step` would destroy that counter's whole
  value, which is its ≤1.00 ceiling — one field could no longer tell a fade
  regression from concealment landing on a decaying note. `note_splice_step` also
  cannot measure a hard join at all: it supplies `w = fade_ramp[OLA_LEN-1] = 1.0`,
  so the residual is a *structural* zero for any arguments. The concealment entry
  goes through `note_conceal_step`, which passes `handover = 0.0`.

### Testing it

The suite is hardware-free and drives the real `fill_output`, so a test sees what
the audio thread sees — including the parts that make the obvious assertion
unfalsifiable. Each of these cost a session:

- **`filtered_buffer_level` is both debited and lagged, so it is never the thing to
  assert on.** The drain debits it from inside `fill_output`
  (`adjust_filtered_level` in [flow.rs](gemacast-core/src/jitter/flow.rs)), so every
  reading a test can see is post-drain and `assert!(peak_filtered > high_limit)`
  fails *precisely when the drain works* — measured peak 2.90 against a live limit
  of 3. Assert on the branch's effect (`tally.accelerated`) instead. It also lags
  the buffer by ~1.3 s, still carrying the prebuffer depth for tens of callbacks
  after the buffer has drained to the edge, so anything gated on
  `filtered < low_limit` (the preemptive growth branch) stays shut across that lag
  and the opening callbacks of a measured window land outside the mechanism under
  test (measured: 4 of 90). Pre-roll in a bounded loop until it converges, *then*
  zero the counters.
- **Every splice leaves a partial frame in `playback_buf`, and it breaks two
  different assertions.** (1) The next callback serves from the deque without
  popping, so expand ratchets occupancy up one frame per splice and a rate-matched
  trickle holds the underrun trigger for only ~25 callbacks — pin the depth
  explicitly or you measure the ratchet instead of the rate limit. (2) `fill_output`
  serves that residue *before* calling `process_next_frame` — measured **480
  samples, exactly half a callback** after setup — so a peak over the whole output
  buffer reads 0.5 of genuine signal even when every newly-generated sample is a
  zero. Capture `manager.playback_buf.len()` before the callback and assert only
  over `output[residue..]` (`pitch_concealment::new_frame_peak`).
- **`TimescaleTally` counters are 1 Hz-windowed and reset**, so an end-of-run read
  is not a total; accumulate per-callback deltas. `log_depth_authority` clears them
  via `LogWindow::rotate` every `LOG_INTERVAL_CALLBACKS` = 100 callbacks, so a measured
  run must fit inside 100 callbacks or zero `manager.log_window.tally` **and**
  `manager.log_window.frame_count` first — otherwise it reads what survived the last
  flush (measured: 16 of 200). `timescale.op_count()` is ambiguous (`note_op()`
  fires from both paths); only `tally.accelerated` / `tally.expanded` are specific.
- **`effective_target` diverges from a setup-time snapshot within a few callbacks**
  (measured 2 vs 4). Recompute limits from `manager.control.effective_target`, never
  from an earlier `target_breakdown().raw`. And **`setup_env()`'s 40 ms config yields
  a degenerate band** — `effective_target` converges to 2, so `buffer_limits(2)` is
  (1, 3) and the below-low branch barely exists; use
  `setup_env_with(JitterConfig { min_depth_ms: 150, .. }, link)` when a test needs a
  real `low_limit` (≈11).
- **Wall-clock does not advance on its own across a test's callbacks**, with two
  consequences. A starvation arms a 500 ms recovery window holding
  `stretch_allowed` off; frozen, it never expires and silently disarms both
  actuators for the rest of the run — so when not driving the clock, drain with
  `while occupied_count() > 1`, never past it. And the wall-clock-gated timers in
  `target.rs` can only be aged by moving the *comparison point* forward, never a
  stored stamp backward: `Instant::now() - Duration` **panics on Windows** when
  uptime is below the subtracted duration (it cost a session — three tests rewound
  3600 s on a box up 2039 s). Manager, `target.rs` and `flow.rs` all take an
  injected `now: Instant`, so call `manager.set_test_clock(now)` (a `#[cfg(test)]`
  hook; release compiles to a bare `Instant::now()`) and build stale timestamps by
  *adding*: `let now = base + Duration::from_secs(3600)`.
- **A ceiling-only assertion passes vacuously when the mechanism never fires.**
  Pair every rate-limit ceiling with a lower bound and a precondition proving the
  trigger was standing.
- **Falsify every new test against the code it replaces, not just against HEAD.**
  v20's two resume tests both pass on v19 *and* on the change unless the
  discriminating case is picked deliberately: one fails only on `target`, the
  other only on `raw_target`, and only `max(target, raw_target)` passes both. v21's
  concealment peak assertions passed vacuously until the same discipline exposed the
  `playback_buf` residue above. v22 generalized it to a **variant matrix** — a fade
  change has two independent halves (slope, floor), so the falsification set is the
  old code *and* each half alone: v21 `/4 ∨ 0.0` fails 5 of 6, floor-only `/4 ∨ 0.15`
  fails the 2 slope discriminators, slope-only `/12 ∨ 0.0` fails the 3 floor ones,
  and only the pair passes all six. Where a change has N independent parts, N+1
  variants is the falsification set, not one. Revert the source and watch it fail —
  what `timescale::tests::crossfade_ramp` documents for the fade shape, generalized.
- **Measure the expected value, do not derive it.** Every v22 assertion came from a
  temporary `panic!("PROBE …")` in the test under construction, run once and then
  replaced. It is what established that `make_uncompressed_packet` reproduces at
  peak **exactly 0.5** on every concealed frame — so `peak / 0.5` *is* the applied
  gain and a schedule can be asserted directly — and that the Opus path cannot be
  asserted that way at all, because `decode_plc` decays on its own (0.501 → 0.007
  across 20 callbacks) and an absolute level there measures the codec, not the fade.
  On the Opus path assert a zero-count (`0 of 20 callbacks entirely silent`), not a
  level.
- **The rebuffer harness's outage length decides which band is under test.** Use
  `rebuffering_at_target_after`; the observed gap is `(stamp − 200 ms)/10 ms`
  frames (last setup arrival is `base + 200 ms`). Anything past ~1.2 s saturates
  `GAP_CLAMP_FRAMES` = 120 and `gap_floor` pins `raw_target` at the 80-frame
  comfort cap — above any burst the ring can hold, so no clamp fires and every
  resume assertion passes against a mechanism that never ran. The v19 default of
  2 s did exactly that once v20 moved the clamp onto `raw_target`.
- **A test cannot prime `max_gap` by hand ahead of a burst.** `record_gap` keeps
  its bucket's *maximum* and the gap is recorded by the packet that ends it, so a
  `stats.record_gap(...)` before `ingest_packets` is overwritten by the burst's
  own arrival. Set the gap through the arrival timeline, and read `raw_target`
  *after* ingest — the order `fill_output` sees them in.

## Audio-path rules

- **Green tests do not mean the audio is better.** Unit tests catch logic
  regressions; they cannot hear a stutter. Any jitter/latency/masking change must
  be confirmed with real-device `adb logcat` captures before it is called fixed.
  v21 established the five-link sweep as the standard scope — ADB, USB tether,
  5 GHz, 2.4 GHz, and 2.4 GHz at 128 kbps — which is what made the 2.4 GHz
  worsening attributable to the link instead of to the round. Past field logs are
  in `LOGS/`.
- **One behavioral change per commit**, so a bad round can be bisected. A field
  capture cannot be bisected at all — it attributes by ear to a whole round — so
  two changes through the same code path in one round are unattributable no
  matter how clean the commits are.
- **The bitrate axis was measured in v18 and is closed.** Four captures, 5 GHz and
  2.4 GHz × uncompressed and 128 kbps Opus (`LOGS/log-*-changesv18.txt`, graded in
  `TEMP/v19-plan.md`). **128 kbps beats uncompressed on both bands** — 5 GHz target
  96 vs 130 ms and starvation 552 vs 2775 ms; 2.4 GHz starvation 2149 vs 6355 ms at
  near-equal targets (299 vs 279 ms). NCC acceptance is uniformly *lower* on
  uncompressed, the opposite of the worry that a sparser decode would correlate
  worse, so **no constant here needs a bitrate split.** Unmeasured: bitrates *below*
  128 kbps.
- **Callback-thread stalls are a scheduler problem — do not chase them from inside
  this module.** `window_ms > 1500` reads 0/438 and 0/507 on the v20 captures and
  23/554 and 12/456 on v21's, tracking the link rather than any commit. It is a
  covariate to report alongside a capture, never a target to tune against.
- **Never remove or downgrade logging in the audio path.** A logging blackout once
  hid a regression for an entire session, and v21 had to establish which *format* a
  capture ran by reading the saturation behaviour of an unrelated heartbeat, because
  nothing recorded it — hence `fmt=` on the depth line. v22 added `floor_frames=`
  for the same reason: it is the only field that can price the stuck-note risk the
  concealment floor accepts. Two counters there read differently than their names
  suggest:
  - **`frames_played` counts callbacks, not packets.** The accelerate staging pop
    takes a *second* packet inside the same callback, so a naive `arrivals - played`
    reads as loss — 6.8% of the 2.4 GHz-uncompressed stream in v18, where true loss
    was 0%. v19's unplayed-frame ledger subtracts `staged_pops`, the term that closes
    `arrivals - played - staged == occupied`.
  - **`avg_rms=` / `max_rms=` are real decoded-PCM RMS on both paths** —
    `get_rms(&decoder.decode_buf[..decode_len])` in `manager.rs`. The packet-size
    proxy is real but feeds a *different* sink: `compute_rms`
    ([packet.rs:37](gemacast-core/src/stream/player/packet.rs#L37)) returns
    `(payload_len / typical_max).sqrt()` on the Opus path and reaches only the
    `RustStdoutStderr: Latency: …ms RMS: …` heartbeat from the receive thread. That
    saturation is what identified the v20 wire formats (unc mean 0.177, **0/432**
    ≥0.95; 128k **496/506** ≥0.95) — a distribution gap that happened not to overlap
    and is not a property to rely on twice.
- On Android, `tracing::*` reaches logcat only because `gemacast-mobile` enables
  the `tracing/log` feature and `tauri-plugin-log` installs the `log` sink. Do
  **not** install a `tracing` subscriber in the mobile crate — it silences the
  bridge.

## Platform / dependency notes

Platform audio backends are declared **only** in `gemacast-core`: `oboe`
(Android), `pipewire` (Linux), `windows`/WASAPI (Windows), `cpal` (everywhere).
`gemacast-mobile` and `gemacast-pc` cannot import them directly — add the wrapper
or probe in `gemacast-core` and re-export. Check the owning `Cargo.toml` before
writing a `use` for a third-party crate.

**macOS capture is live, not disabled** — that claim was stale for several
releases. [adapters/capture/mod.rs:247](gemacast-core/src/adapters/capture/mod.rs#L247)
gates on `macos_supports_sck()` (product version major `>= 13`, cached in a
`OnceLock`) at two call sites, desktop and per-process: on macOS 13+ it takes
ScreenCaptureKit (`sck_common` / `sck_desktop` / `sck_process`), and below 13 —
or on *any* SCK failure — it falls back to CPAL loopback. **CPAL loopback
captures nothing on a stock Mac**; it needs a virtual output device (BlackHole,
Soundflower), which is why the fallback logs an install-or-upgrade hint once per
session rather than failing.

Two different TCC gates, and only one of them is an Info.plist concern:

| path | permission | how it's requested |
| --- | --- | --- |
| SCK (primary, macOS 13+) | Screen Recording | system-generated prompt — **no Info.plist purpose string exists for it**; a denial arrives as the typed `SCError::PermissionDenied`, mapped to `AudioError::ScreenCapturePermissionDenied` so the UI can prompt |
| CPAL loopback (fallback) | Microphone | `NSMicrophoneUsageDescription` in [macos/Info.plist](macos/Info.plist) — required because `build_input_stream` opens a CoreAudio *input* device |

`gemacast-pc` is a `tao` tray app, **not** a Tauri app, so its bundle does not
come from `tauri.conf.json` — `macos/Info.plist` is templated into
`Contents/Info.plist` by
[create-macos-app-bundles.sh:22](.github/scripts/create-macos-app-bundles.sh#L22),
which is the only place `VERSION_PLACEHOLDER` is substituted. Edit the plist
there, not in any Tauri config.

macOS artifacts ship **unsigned and un-notarized** by deliberate choice — the
release pipeline contains no `codesign` or `notarytool` step and no `APPLE_*`
secrets. Gatekeeper will therefore refuse the first launch, and users need the
right-click → Open path (or `xattr -d com.apple.quarantine`). That has to be
documented wherever install instructions live.

Self-update is shared: [updater/mod.rs](gemacast-core/src/updater/mod.rs) fetches
an `updater.json` manifest (3 retries, doubling backoff from 1 s), then each app
installs its own way — `tasks/updater.rs` → a tray `TrayEvent::UpdateReady` on PC,
`services/updater/install.rs` → an APK intent on Android.

**The sha256 digest is mandatory and validated at the trust boundary — it used to
be optional, and that was a fail-open integrity check.** `PlatformEntry.sha256` is
still `Option<String>` at the serde layer, but only so a legacy or malformed entry
fails with a clear message instead of an opaque parse error:
`PlatformEntry::verified_sha256` rejects a missing, empty, or non-64-hex value, and
`check_for_update` calls it, so `UpdateInfo.sha256` is a plain `String` and
`download_update` takes `&str` and verifies unconditionally. Both degenerate
shapes were real pipeline output, not hand-edited manifests — entries predating the
field omitted it, and `generate-updater.sh` emitted `sha256: ""` when it could not
fetch an artifact. The old code skipped verification entirely for the first shape,
i.e. the check was gated on a field controlled by the same document it protects.
The non-optional types are the fix and are load-bearing all the way through the
Tauri command (`download_update(… sha256: String)`) and
[tauri-bridge.ts](gemacast-mobile/src/core/tauri-bridge.ts) — the digest round-trips
through JavaScript, so widening either back to nullable silently restores the
unverified install path.

`generate-updater.sh` now **omits** a platform it cannot download and hash, rather
than publishing it digest-less, and fails outright if that leaves zero platforms.
This is a reachable path, not a hypothetical: `installer-tests-gate` counts a
`skipped` test job as a pass, so a failed `build-android` lets the release finish
without an APK. Omitted, the client reports "no entry for platform" and offers no
update; digest-less, it used to download and then reject with a confusing
`expected , got <hash>`.

The `signature` field points at a real detached GPG signature (`sign-release-assets.sh`
uploads them), but **nothing verifies it in-app** — there is no pinned public key in
the binary. In-app integrity is the digest plus HTTPS on the manifest, which covers
a corrupted download and artifact substitution but *not* modification of the
manifest itself. Do not read the field's presence as a guarantee.

## Conventions

- Test names are full sentences:
  `should_output_silence_while_prebuffering_until_target_depth`,
  `rebuffer_pause_collapses_a_starvation_cluster_into_one_event`. Group them in
  nested `mod` blocks by concern.
- Tests touching a global daemon use `serial_test`, all 7 in the named group
  `#[serial(pipewire)]` (Linux capture adapters + `process_lister`). The rest of
  the suite is hardware-free by construction and runs fully parallel.
- Mocks live in a `testing.rs` per crate, each `#[cfg(test)]`-gated — including
  `gemacast-core`'s, so the app crates **cannot** reuse the core mocks and each
  keeps its own. A new port needs its mock added in the same change, or the
  services depending on it stop being testable.
- Tuning constants carry a comment stating the measurement or field log that
  justifies the value. Preserve it when changing one.
- Versions are managed by release-please (`.release-please-manifest.json`);
  `package.json` and `tauri.conf.json` are synced from `Cargo.toml` at release.
