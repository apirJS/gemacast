# Contributing to Gemacast

Thank you for improving Gemacast. Keep changes focused, preserve protocol and
platform behavior unless the change explicitly targets them, and include tests
for observable behavior.

Read [CLAUDE.md](CLAUDE.md) for the repository's architecture and coding rules.

## Toolchain

| Tool | Version |
| --- | --- |
| Rust | Pinned by `rust-toolchain.toml` |
| Bun | 1.x; CI currently uses 1.3.14 |
| Clang and CMake | Required to build the vendored Opus library |
| Java | 17 for Android |
| Android NDK | 25.2.9519653 |
| Android CMake | 3.22.1 |
| Android build-tools | 34.0.0 |
| Android platform | 36 |

Install `cargo-ndk` before running Android Rust checks:

```bash
cargo install cargo-ndk
```

## Clone and install

```bash
git clone https://github.com/apirJS/gemacast.git
cd gemacast

cd gemacast-mobile
bun install --frozen-lockfile

cd ../web
bun install --frozen-lockfile
```

### Linux dependencies

Linux development requires PipeWire, WirePlumber, ALSA, GTK3, WebKit2GTK, and
AppIndicator development packages. The CI workflow is the authoritative setup
for its Ubuntu runners because Ubuntu 22.04's packaged PipeWire headers are too
old for the current Rust bindings.

Common Debian/Ubuntu packages:

```bash
sudo apt-get install -y clang cmake pkg-config ninja-build meson \
  libasound2-dev libpipewire-0.3-dev libgtk-3-dev \
  libayatana-appindicator3-dev libwebkit2gtk-4.1-dev \
  librsvg2-dev patchelf libxdo-dev libudev-dev libdbus-1-dev
```

Common Fedora packages:

```bash
sudo dnf install clang cmake pkg-config ninja-build meson \
  alsa-lib-devel pipewire-devel gtk3-devel \
  libayatana-appindicator-gtk3-devel webkit2gtk4.1-devel \
  librsvg2-devel patchelf libxdo-devel systemd-devel dbus-devel
```

## Build

### PC application

From the repository root:

```bash
cargo build --release -p gemacast-pc
```

The executable is written to `target/release/gemacast-pc`, with `.exe` added on
Windows.

### Android application

From `gemacast-mobile/`:

```bash
# Universal APK
bunx tauri android build --apk

# Smaller ARM64 APK
bunx tauri android build --apk --target aarch64 --split-per-abi
```

Android output is written below
`gemacast-mobile/src-tauri/gen/android/app/build/outputs/apk/`.

The tracked files under `gemacast-mobile/src-tauri/gen/android/` include
hand-written Kotlin. Do not delete or wholesale-regenerate that directory.

### Frontend development

From `gemacast-mobile/`:

```bash
bun run dev
```

From `web/`:

```bash
bun dev
bun run build
```

## Validate

Start with the narrowest relevant test, then run the owning suite.

### Rust

From the repository root:

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Linux workspace tests require live PipeWire and WirePlumber services. See
`.github/workflows/ci.yml` for the headless CI setup.

### Mobile frontend

From `gemacast-mobile/`:

```bash
bun run format:check
bun run lint
bun run typecheck
bun test
```

### Website

From `web/`:

```bash
bun run type-check
bun run lint
bun test:unit --run
bun run build
```

### Android

From the repository root:

```bash
cargo ndk -t arm64-v8a clippy --workspace --exclude gemacast-pc -- -D warnings
```

Then, from `gemacast-mobile/src-tauri/gen/android/`:

```bash
bash ./gradlew :app:testUniversalDebugUnitTest --no-daemon
```

Shell scripts and `gradlew` must retain LF line endings.

### Containers

Linux desktop and Android checks can run together:

```bash
docker compose -f docker/compose.yaml --profile android up --build --abort-on-container-exit
```

Windows and macOS validation use native CI runners and cannot be reproduced by
the Linux containers.