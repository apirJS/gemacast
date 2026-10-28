# Container checks

[compose.yaml](compose.yaml) defines two Linux containers:

| Service | Checks | Entry point |
| --- | --- | --- |
| `test` | Cargo audit, mobile frontend checks, Rust formatting, Clippy, workspace tests | [ci.sh](ci.sh) |
| `android` | Rust formatting, ARM64 Android Clippy, Kotlin JVM tests | [android-ci.sh](android-ci.sh) |

The `test` container starts PipeWire and WirePlumber in a D-Bus session.
The `android` container configures the SDK/NDK and generates Tauri/Wry support
files before Gradle tests. Windows and macOS checks use native CI runners.

## Commands

Run from the repository root with Docker configured for Linux containers.

Build both images:

```sh
docker compose -f docker/compose.yaml --profile android build test android
```

Run each suite:

```sh
docker compose -f docker/compose.yaml run --rm test
docker compose -f docker/compose.yaml --profile android run --rm android
```

The PowerShell runner executes both suites concurrently and waits for both:

```powershell
./docker/test-parallel.ps1
```

## Storage

The repository is mounted at `/work`. Cargo targets and registries, Android
Gradle caches, and mobile `node_modules` use named volumes. Generated files
outside those volumes are written to the host checkout.
