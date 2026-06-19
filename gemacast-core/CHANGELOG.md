# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0](https://github.com/apirJS/gemacast/releases/tag/gemacast-core-v0.1.0) - 2026-06-10

### Bug Fixes

- *(core)* improve connection reliability and thread teardown
- *(network)* Fix USB vs WIFI naming checks. Stopping sending presence on 'Stop Broadcast'
- *(network)* resolve discovery, graceful disconnects, and mobile timeout logic

### Features

- *(gemacast-core)* mDNS for discovery
- *(gemacast-mobile)* Play/Pause stream functionality
- Each Receiver can have their own bitrate quality
- Enable users to manually input the PC address
- Resampler for PC-side capture with rubato
- Proces-Level Loopback Capture on Windows
- massive refactor
- Introducing preset options + custom preset for the Jitter Management Config, added settings panel drawer, improved reconnection mechanism, improved Jitter Management algorithm, improved discovery mechanism
- More tolerant Jitter Manager
- Migrate from cpal to Oboe (Low latency mode) for Android
- Bitrate option for user and Adaptive Jitter Buffer
- foreground service, usb tether support, media session control, dynamic buffer on the sender side
- shift to static jitter buffer, robust volume controls, and presence updates
- Added Jitter Handling
- *(audio)* added PLC

### Refactoring

- *(gemacast-core)* Separating concerns of core into Discovery, Control, and Stream
- *(gemacast-core)* Split sender.rs and receiver.rs into several files as modules
- *(mobile)* Separate css to serveral files, making dom handling and state handling more modular
- Changing the discovery mechanism from phone-pc to pc-phone

### Chore

- added tracing logging in crucial points

### Style

- fix rustfmt issues

### Test

- *(gemacast-core)* more tests
