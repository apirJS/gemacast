# Container tests

These images mirror the Linux desktop and Android jobs in
`.github/workflows/ci.yml`. Windows and macOS tests remain on native runners.

Run Linux and Android concurrently:

```sh
docker compose -f docker/compose.yaml --profile android up --build --abort-on-container-exit
```

Run one service:

```sh
docker compose -f docker/compose.yaml run --rm test
docker compose -f docker/compose.yaml --profile android run --rm android
```

Build without running tests:

```sh
docker compose -f docker/compose.yaml --profile android build test android
```

Source is mounted at `/work`; Cargo, Gradle, and frontend dependencies use
named volumes to avoid conflicts with host artifacts.

Windows containers require a separate Windows Docker engine and cannot run
beside these Linux containers. macOS applications cannot run in Docker.
