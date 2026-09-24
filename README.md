<div align="center">

# <img src="assets/logo_transparent.svg" height="56" alt="Gemacast logo" valign="middle" /> Gemacast

[![Release](https://img.shields.io/github/v/release/apirJS/gemacast?label=release)](https://github.com/apirJS/gemacast/releases/latest)
[![Rust](https://img.shields.io/badge/Rust-000000?logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![TypeScript](https://img.shields.io/badge/TypeScript-3178C6?logo=typescript&logoColor=white)](https://www.typescriptlang.org/)
[![Downloads](https://img.shields.io/github/downloads/apirJS/gemacast/total)](https://github.com/apirJS/gemacast/releases)
[![License](https://img.shields.io/github/license/apirJS/gemacast)](LICENSE)

Stream desktop or per-application audio from your PC to one or more Android
phones over Wi-Fi, USB tethering, or ADB. Every phone can use its own audio
source, bitrate, and buffer settings.

[Download the latest release](https://github.com/apirJS/gemacast/releases/latest)

</div>

## Quick start

1. Install the Gemacast PC app for Windows, Linux, or macOS from
   [Releases](https://github.com/apirJS/gemacast/releases/latest).
2. Install `gemacast-mobile.apk` on an ARM64 phone. Install
   `gemacast-mobile-universal.apk` when you need the multi-architecture build.
3. Start Gemacast on the PC, then open it on the phone.
4. Select a connection mode on the phone and choose the discovered PC.
5. For a first-time LAN connection, confirm that both devices show the same
   six-digit pairing code and approve the connection.

The PC app runs in the system tray. If the phone cannot discover it over Wi-Fi
or USB tethering, check the [firewall ports](#firewalls).

## Connection modes

| Mode | Setup | Notes |
| --- | --- | --- |
| Wi-Fi | Put both devices on the same non-guest network and select **Wi-Fi**. | Uses the local wireless network; latency depends on that network. |
| USB tethering | Connect the cable, enable **USB tethering** in Android settings, then select **USB**. | Creates a network interface over USB and uses the LAN protocol. |
| ADB | Enable Android developer options and USB debugging, connect the cable, approve the computer, then select **ADB**. | The PC process configures `adb reverse`; no LAN firewall rule is used. |

VPNs, guest Wi-Fi, and router client isolation can prevent LAN discovery even
when both devices have a strong signal.

## Screenshots

<div align="center">
  <img src="assets/mobile-stream-adb-demo.gif" alt="Gemacast streaming PC audio to an Android phone" height="480" />
</div>

<details>
<summary>More screenshots</summary>
<br />
<div align="center">
  <img src="assets/stream-choose-process-audio-demo.jpeg" alt="Selecting a per-application audio source" height="460" />
  <img src="assets/setting-panel-1.jpeg" alt="Gemacast mobile settings" height="460" />
  <br /><br />
  <img src="assets/setting-panel-2.jpeg" alt="Additional Gemacast mobile settings" height="460" />
  <br /><br />
  <img src="assets/pc-system-tray.png" alt="Gemacast PC system tray" width="720" />
</div>
</details>

## Features

- Full desktop or per-application audio capture.
- Multiple phones with independent source, bitrate, and buffer settings.
- Opus LowDelay/CELT from 6 to 512 Kbps, or uncompressed stereo PCM.
- Adaptive and fixed jitter-buffer presets.
- Wi-Fi, USB tethering, and automatic ADB forwarding.
- LAN control channel with TLS, P-256 device authentication, and a six-digit
  comparison code.
- Optional PC-volume synchronization and automatic reconnection.

## Implemented platforms

| PC platform | Capture implementation | Automated CI coverage |
| --- | --- | --- |
| Windows | WASAPI desktop and per-process loopback | `windows-latest` |
| Linux | PipeWire desktop and per-process capture; cpal desktop fallback | Ubuntu 22.04 with PipeWire 1.2.7 and WirePlumber 0.5.15 |
| macOS 13+ | ScreenCaptureKit desktop and per-process capture; cpal desktop fallback | macOS 15 |
| macOS 11-12 | Code selects the cpal desktop fallback; per-process capture is unavailable | Not covered by CI |

The player requires Android 8.0 (API 26) or later. iOS is not currently
supported.

## Installation

### Windows

Download the `.msi` installer or `.zip` archive from
[Releases](https://github.com/apirJS/gemacast/releases/latest). The MSI installs
TCP and UDP firewall exceptions for `gemacast-pc.exe`. Portable installations
do not create those exceptions.

### Linux

PipeWire is used for desktop and per-process capture. Without PipeWire, the code
can fall back to cpal for desktop capture, but per-process capture is
unavailable. Download the `.deb`, `.rpm`, `.AppImage`, or `.tar.xz` package from
[Releases](https://github.com/apirJS/gemacast/releases/latest).

```bash
# Debian / Ubuntu, with dependency resolution
sudo apt install ./gemacast-pc_*.deb

# Fedora / RHEL, with dependency resolution
sudo dnf install ./gemacast-pc-*.rpm
```

The packages declare their UI dependencies. Their maintainer scripts make a
best-effort attempt to configure supported firewalls. AppImage and archive
installations do not configure the firewall. The archive requires GTK3 and
AppIndicator, such as `libayatana-appindicator3-1`, for the tray icon.

### macOS

Download the `.dmg` from
[Releases](https://github.com/apirJS/gemacast/releases/latest). The app is
currently unsigned and unnotarized. On first launch, right-click the app and
select **Open**, or remove its quarantine attribute:

```bash
xattr -d com.apple.quarantine /Applications/Gemacast.app
```

On macOS 13 and later, ScreenCaptureKit capture requires Screen Recording
permission. The code selects the cpal fallback on macOS 11-12 or after a
ScreenCaptureKit failure. Capturing system output through that fallback requires
routing the output to a capture device, such as BlackHole or Soundflower. The
macOS 11-12 path is not covered by CI.

### Android

Download and install the APK from
[Releases](https://github.com/apirJS/gemacast/releases/latest). Use
`gemacast-mobile.apk` for the ARM64 build or
`gemacast-mobile-universal.apk` for the multi-architecture build.

## Security and privacy

- LAN control traffic uses TLS, device authentication, and per-device session
  tokens that rotate when a device reconnects.
- The six-digit code lets you confirm that the phone is pairing with the PC you
  expect.
- UDP audio on Wi-Fi and USB tethering is not encrypted or authenticated. Those
  transports provide no audio-packet confidentiality or integrity.
- Use Wi-Fi and USB tethering only on a trusted network. ADB carries the stream
  through its USB-debugging connection instead of the LAN.
- Per-application capture reveals the PC's process list to an authenticated
  phone so that the user can select a source.

## Firewalls

Wi-Fi and USB tethering require these inbound PC ports:

| Port | Protocol | Purpose |
| --- | --- | --- |
| 23555 | UDP | Discovery |
| 23556 | UDP | Audio stream |
| 23559 | TCP | TLS control |

ADB uses loopback TCP ports 23557 and 23558 through `adb reverse` and requires
no firewall rule.

Windows MSI installations configure the firewall automatically. For Linux:

```bash
# ufw
sudo ufw allow 23555,23556/udp
sudo ufw allow 23559/tcp

# firewalld
sudo cp linux/gemacast.firewalld.xml /usr/lib/firewalld/services/gemacast.xml
sudo firewall-cmd --reload
sudo firewall-cmd --permanent --add-service=gemacast
sudo firewall-cmd --reload
```

On macOS, allow incoming connections when prompted.

## Audio formats

| Format | Codec | Bitrate | Packet duration | Frame contents |
| --- | --- | --- | --- | --- |
| Opus | Opus LowDelay/CELT | 6-512 Kbps | 10 ms | 480 stereo frames / 960 interleaved samples |
| Uncompressed | Raw `f32` stereo PCM | About 3.072 Mbps | 10 ms | 480 stereo frames / 960 interleaved samples |

All formats use 48 kHz stereo. Sources with another sample rate are resampled
before transmission.

## FAQ

<details>
<summary><strong>The phone cannot find my PC</strong></summary>

Both devices must be on the same reachable network. Guest Wi-Fi, VPNs, and
router client isolation can hide the PC. Confirm that Gemacast is running and
that UDP 23555-23556 and TCP 23559 are allowed through the PC firewall. ADB
mode does not use LAN discovery or firewall rules.

</details>

<details>
<summary><strong>What is the real end-to-end latency?</strong></summary>

End-to-end latency includes PC capture and packetization, one-way transport, the
phone's jitter buffer, decoding, and the phone's audio-output buffer. The phone
reports round-trip time and jitter-buffer depth. RTT does not determine one-way
delay unless the path is symmetric, and the app does not measure capture or
hardware-output latency, so those metrics are not a complete end-to-end value.

</details>

<details>
<summary><strong>The buffer grows past 200 ms when the screen turns off</strong></summary>

Some Android devices change Wi-Fi scheduling when the screen turns off. If
packets begin arriving in larger batches, the adaptive buffer grows in response.
**Keep Screen On** avoids the screen-off state; USB tethering and ADB avoid the
Wi-Fi path.

</details>

<details>
<summary><strong>Why is there a pairing step?</strong></summary>

Pairing decides which phones may control the PC, request its process list, and
select what it captures. After approval, the phone stores the PC identity.
Automatic reconnection depends on the app's **Auto Reconnect** setting.

</details>

<details>
<summary><strong>What is the six-digit code for?</strong></summary>

Both devices independently derive the code from the authenticated pairing
exchange. Matching codes provide a human check against a man-in-the-middle
connection. Compare both screens before approving; the code is a verification
value, not a password.

</details>

<details>
<summary><strong>Is the audio encrypted?</strong></summary>

The control channel is encrypted and authenticated. UDP audio sent over Wi-Fi
or USB tethering is not encrypted or authenticated; the protocol provides no
confidentiality or integrity for those packets. ADB carries the stream through
its loopback TCP forwarding path.

</details>

## Development

See [CONTRIBUTING.md](CONTRIBUTING.md) for toolchain setup, build commands,
tests, architecture rules, and pull-request guidance.

## License and acknowledgements

Gemacast is licensed under [GPL-3.0-or-later](LICENSE). See
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) for major third-party libraries
and their licenses.
