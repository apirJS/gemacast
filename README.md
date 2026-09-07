<div align="center">

# <img src="assets/logo_transparent.svg" height="56" alt="Gemacast Logo" valign="middle" /> Gemacast

[![Release](https://img.shields.io/github/v/release/apirJS/gemacast?label=release)](https://github.com/apirJS/gemacast/releases/latest)
[![Rust](https://img.shields.io/badge/rust-1.97.1-orange)](https://www.rust-lang.org/)
[![TypeScript](https://img.shields.io/badge/typescript-6-blue)](https://www.typescriptlang.org/)
[![Release Date](https://img.shields.io/github/release-date/apirJS/gemacast)](https://github.com/apirJS/gemacast/releases/latest)
[![Last Commit](https://img.shields.io/github/last-commit/apirJS/gemacast)](https://github.com/apirJS/gemacast/commits/main)
[![Downloads](https://img.shields.io/github/downloads/apirJS/gemacast/total)](https://github.com/apirJS/gemacast/releases)
[![License](https://img.shields.io/github/license/apirJS/gemacast)](LICENSE)
[![Website](https://img.shields.io/badge/website-gemacast.apirjs.tech-blue)](https://gemacast.apirjs.tech/)

Stream desktop audio from PC to Android over Wi-Fi, USB tethering, or ADB.
Captures full audio or per-application audio and plays it on one or more phones.<br>Turn your phone into a speaker!!

</div>

## Table of Contents

- [Screenshots](#screenshots)
- [Requirements and Setup](#requirements-and-setup)
- [Features](#features)
- [Firewalls](#firewalls)
- [Audio Formats](#audio-formats)
- [Compile from Source](#compile-from-source)
- [FAQ](#faq)
- [License](#license)
- [Third-Party Library Acknowledgement](#third-party-library-acknowledgement)

## Screenshots

<div align="center">
  <img src="assets/mobile-stream-adb-demo.gif" alt="Phone playing" height="480" />
  <img src="assets/stream-choose-process-audio-demo.jpeg" alt="Choose Process Audio" height="480" />
  <br /><br />
  <img src="assets/setting-panel-1.jpeg" alt="Phone settings 1" height="480" />
  <img src="assets/setting-panel-2.jpeg" alt="Phone settings 2" height="480" />
  <br /><br />
  <img src="assets/pc-system-tray.png" alt="PC system tray" width="720" />
</div>

## Requirements and Setup

### Windows

Minimum: Windows 10 version 2004 or later.

1. Download the `.msi` installer or `.zip` archive from [Releases](https://github.com/apirJS/gemacast/releases/latest).
2. Run the installer. It creates a Windows Firewall rule for ports UDP 23555-23556 and TCP 23559.
3. Launch Gemacast from the Start menu or system tray.

### Linux

Requires a PipeWire 0.3+ session with WirePlumber for audio capture.

Download `.deb`, `.rpm`, `.AppImage`, or `.tar.xz` from [Releases](https://github.com/apirJS/gemacast/releases/latest).

```bash
# Debian / Ubuntu (.deb)
sudo dpkg -i gemacast-pc_*.deb

# Fedora / RHEL (.rpm)
sudo rpm -i gemacast-pc-*.rpm
```

The `.deb` and `.rpm` packages automatically install required UI libraries (GTK3, AppIndicator) and configure firewall rules and port reservations.

For `.AppImage` or `.tar.xz`, you must open ports manually (see [Firewalls](#firewalls)). Additionally, the `.tar.xz` binary requires GTK3 and AppIndicator (e.g., `libayatana-appindicator3-1`) installed on your system to display the system tray icon.

### macOS

Requires macOS >= 13

Download the `.dmg` from [Releases](https://github.com/apirJS/gemacast/releases/latest).

The binary is unsigned and un-notarized. On first launch, right-click the app and select Open, or run:

```bash
xattr -d com.apple.quarantine /Applications/Gemacast.app
```

Audio capture uses ScreenCaptureKit on macOS 13 and later (requires Screen Recording permission).
On macOS 12 or below, CPAL is used as a fallback and requires a virtual output device (BlackHole or Soundflower).

### Android

Minimum: Android 8.0 (Oreo, API 26) or later.

Download the `.apk` from [Releases](https://github.com/apirJS/gemacast/releases/latest) and install it.
The phone and PC must be on the same network, connected via USB tethering, or linked by an ADB cable with `adb reverse` forwarding.

## Features

| Feature | Details |
|---|---|
| Audio capture | Full desktop audio or per-application audio |
| Codecs | Opus LowDelay/CELT at 10-512 kbps, or uncompressed PCM at 48 kHz stereo |
| Multi-device | Stream to multiple Android devices simultaneously with independent settings |
| Secure pairing | TLS control channel with ECDSA P-256 device auth and 6-digit pairing code |
| Adaptive jitter buffer | Per-client buffer depth adjusted to link quality on the audio thread |

## Firewalls

Gemacast uses three inbound ports on the PC. All three must be reachable from the phone's network for Wi-Fi and USB tethering connections.

| Port  | Protocol | Purpose            |
|-------|----------|--------------------|
| 23555 | UDP      | Discovery          |
| 23556 | UDP      | Audio stream       |
| 23559 | TCP      | TLS control (HTTPS)|

ADB connections (TCP 23557, 23558) run over loopback via `adb reverse` and require no firewall rule.

**Windows**: The MSI installer creates rules automatically. For portable installs, allow the ports through Windows Firewall.

**Linux (ufw)**:
```bash
sudo ufw allow 23555,23556/udp
sudo ufw allow 23559/tcp
```

**Linux (firewalld)**:
```bash
sudo cp linux/gemacast.firewalld.xml /usr/lib/firewalld/services/gemacast.xml
sudo firewall-cmd --reload
sudo firewall-cmd --permanent --add-service=gemacast
sudo firewall-cmd --reload
```

**macOS**: Allow incoming connections when the system dialog appears on first launch.

## Audio Formats

| Format       | Codec               | Bitrate          | Frame Size | Latency per Frame |
|--------------|---------------------|------------------|------------|--------------------|
| Opus         | Opus LowDelay/CELT  | 10-512 kbps      | 480 samples | 10 ms             |
| Uncompressed | Raw PCM (f32 stereo)| ~3072 kbps       | 480 samples | 10 ms             |

All formats run at 48 kHz stereo. The capture pipeline resamples any source rate to 48 kHz before encoding.

## Compile from Source

### Prerequisites

| Tool | Version | Notes |
|---|---|---|
| Rust | 1.97.1 | Pinned in `rust-toolchain.toml` |
| Clang | any | Required by `audiopus_sys` (Opus C build) |
| CMake | 3.x | Required by `audiopus_sys` (Opus C build) |
| Bun | 1.x | Frontend build for the mobile app |
| Java | 17 (Temurin) | Android target only |
| Android SDK + NDK | NDK 25.2.9519653 | Android target only |
| cargo-ndk | latest | Android target only (`cargo install cargo-ndk`) |

Linux requires PipeWire, ALSA, GTK3, WebKit2GTK, and related development headers:

```bash
# Debian / Ubuntu
sudo apt-get install -y clang cmake pkg-config ninja-build meson \
  libasound2-dev libpipewire-0.3-dev libgtk-3-dev \
  libayatana-appindicator3-dev libwebkit2gtk-4.1-dev \
  librsvg2-dev patchelf libxdo-dev libudev-dev libdbus-1-dev

# Fedora
sudo dnf install clang cmake pkg-config ninja-build meson \
  alsa-lib-devel pipewire-devel gtk3-devel \
  libayatana-appindicator-gtk3-devel webkit2gtk4.1-devel \
  librsvg2-devel patchelf libxdo-devel systemd-devel dbus-devel
```

### Build PC (Windows / Linux / macOS)

```bash
git clone https://github.com/apirJS/gemacast.git
cd gemacast
cargo build --release -p gemacast-pc
```

The binary is written to `target/release/gemacast-pc` (or `gemacast-pc.exe` on Windows).

### Build Android

```bash
cd gemacast/gemacast-mobile
bun install --frozen-lockfile
bunx tauri android build --apk
```

The unsigned APK is written to `gemacast-mobile/src-tauri/gen/android/app/build/outputs/apk/universal/release/`.

## FAQ

<details>
<summary><b>The phone cannot find my PC</b></summary>
<br/>

Both devices have to be on the same network, so a guest Wi-Fi or a VPN will hide your PC even at full signal. The firewall has to let Gemacast through, which is ports UDP 23555 (for presence), UDP 23556 (for audio stream) and TCP 23559 (for controls). Windows asks you about this once on the first launch, so it stays blocked if you closed that popup.

</details>

<details>
<summary><b>What is the real end-to-end latency?</b></summary>
<br/>

Add up three things: 10 ms to record the audio on PC, half the round trip (RTT) to send it, and whatever the buffer is holding. The buffer is usually the biggest part. The phone shows you the buffer and the round trip while it plays, so you can add up your own number.

</details>

<details>
<summary><b>The buffer grows past 200 ms when the screen turns off</b></summary>
<br/>

Android is saving battery. With the screen off it puts the Wi-Fi chip to sleep and only wakes it up on a set schedule, called DTIM. Audio stops arriving in a steady trickle and starts landing in batch, 100 to 200 ms apart. The buffer grows to cover the longest gap it sees, and that is the expected behavior, because a smaller buffer would just stutter the stream. To keep it low, turn on Keep Screen On in settings, or use USB tether or ADB, where the chip never sleeps.

</details>

<details>
<summary><b>Why is there a pairing step?</b></summary>
<br/>

Without it, anything on your network could start a stream, see a list of your running apps, and change what your PC is recording. With pairing, you can allow which phone to connect. You allow it once, the first time you connect, and after that your phone remembers the PC and connects flawlessly.

</details>

<details>
<summary><b>What is the 6-digit code for?</b></summary>
<br/>

It shows that your phone is talking straight to your PC, with nothing in between (no man-in-middle). Each device works out the code by itself, using details only those two share. If some other machine were sitting in the middle, the two codes would come out different and you would see it right away. So just check that both screens show the same six digits, then approve. It is not a password, so it does not matter if someone else sees it.

</details>

<details>
<summary><b>Is the audio encrypted?</b></summary>
<br/>

No, and that is on purpose. Pairing is protected, but the audio itself is sent plain. The whole point of this app is low delay, and the audio goes out in tiny pieces, 100 of them every second. Locking and unlocking every one of those adds work at both ends and makes each piece bigger, which is exactly the kind of cost that shows up as delay. So anyone on the same network could listen in if they wanted to. Stick to a network you trust, or use USB.

</details>

## License

[GPL-3.0-or-later](LICENSE)

## Third-Party Library Acknowledgement

### Rust

| Crate | License | Purpose |
|---|---|---|
| [tokio](https://crates.io/crates/tokio) | MIT | Async runtime |
| [axum](https://crates.io/crates/axum) | MIT | HTTP/WebSocket control server |
| [tao](https://crates.io/crates/tao) | Apache-2.0 / MIT | Window and system tray event loop (PC) |
| [tray-icon](https://crates.io/crates/tray-icon) | Apache-2.0 / MIT | System tray icon (PC) |
| [tauri](https://crates.io/crates/tauri) | Apache-2.0 / MIT | Mobile app framework |
| [opus](https://crates.io/crates/opus) ([audiopus_sys](https://crates.io/crates/audiopus_sys)) | MIT / BSD-3 / ISC | Opus audio codec bindings |
| [oboe](https://crates.io/crates/oboe) | Apache-2.0 | Low-latency audio output (Android) |
| [cpal](https://crates.io/crates/cpal) | Apache-2.0 | Cross-platform audio I/O fallback |
| [pipewire](https://crates.io/crates/pipewire) | MIT | PipeWire audio capture (Linux) |
| [screencapturekit](https://crates.io/crates/screencapturekit) | Apache-2.0 / MIT | Screen/audio capture (macOS) |
| [rubato](https://crates.io/crates/rubato) | MIT / Apache-2.0 | Async sample rate converter |
| [ringbuf](https://crates.io/crates/ringbuf) | MIT / Apache-2.0 | Lock-free ring buffer |
| [mdns-sd](https://crates.io/crates/mdns-sd) | Apache-2.0 / MIT | mDNS service discovery |
| [reqwest](https://crates.io/crates/reqwest) | MIT / Apache-2.0 | HTTP client |
| [rustls](https://crates.io/crates/rustls) | Apache-2.0 / MIT / ISC | TLS implementation |
| [ring](https://crates.io/crates/ring) | Apache-2.0 / ISC / OpenSSL | Cryptographic primitives (ECDSA, SHA-256) |
| [rcgen](https://crates.io/crates/rcgen) | Apache-2.0 / MIT | X.509 certificate generation |
| [serde](https://crates.io/crates/serde) / [serde_json](https://crates.io/crates/serde_json) | MIT / Apache-2.0 | Serialization |
| [rfd](https://crates.io/crates/rfd) | MIT | Native file/message dialogs (PC) |
| [image](https://crates.io/crates/image) | MIT / Apache-2.0 | Image processing for tray icons |
| [semver](https://crates.io/crates/semver) | MIT / Apache-2.0 | Semantic versioning for update checks |

### Frontend (TypeScript)

| Package | License | Purpose |
|---|---|---|
| [react](https://www.npmjs.com/package/react) / [react-dom](https://www.npmjs.com/package/react-dom) | MIT | UI framework |
| [zustand](https://www.npmjs.com/package/zustand) | MIT | State management |
| [tailwindcss](https://www.npmjs.com/package/tailwindcss) | MIT | Utility-first CSS |
| [lucide-react](https://www.npmjs.com/package/lucide-react) | ISC | Icon library |
| [vite](https://www.npmjs.com/package/vite) | MIT | Build tool and dev server |
| [@tauri-apps/api](https://www.npmjs.com/package/@tauri-apps/api) | Apache-2.0 / MIT | Tauri IPC bridge |

### Android (Kotlin)

| Library | License | Purpose |
|---|---|---|
| [Android Keystore API](https://developer.android.com/training/articles/keystore) | Apache-2.0 | Hardware-backed ECDSA key storage |
