#!/usr/bin/env bash
# Run the Android backend checks from ci.yml inside the Android test image.
set -euo pipefail

if [ -f /etc/gemacast-android-env ]; then
  # shellcheck disable=SC1091
  . /etc/gemacast-android-env
  export ANDROID_NDK_HOME ANDROID_NDK_ROOT NDK_HOME BUILD_TOOLS_VERSION
fi

export TAURI_ANDROID_PROJECT_PATH="${TAURI_ANDROID_PROJECT_PATH:-/work/gemacast-mobile/src-tauri/gen/android}"
export WRY_ANDROID_PACKAGE="${WRY_ANDROID_PACKAGE:-com.apir.gemacast}"
export WRY_ANDROID_LIBRARY="${WRY_ANDROID_LIBRARY:-gemacast_mobile_lib}"
export WRY_ANDROID_KOTLIN_FILES_OUT_DIR="${WRY_ANDROID_KOTLIN_FILES_OUT_DIR:-/work/gemacast-mobile/src-tauri/gen/android/app/src/main/java/com/apir/gemacast/generated}"

mkdir -p "$WRY_ANDROID_KOTLIN_FILES_OUT_DIR"
cd /work

echo "=== Android: cargo fmt ==="
cargo fmt --check

echo "=== Android: cargo-ndk clippy ==="
cargo ndk -t arm64-v8a clippy --workspace --exclude gemacast-pc -- -D warnings

echo "=== Android: Gradle JVM tests ==="
cd /work/gemacast-mobile/src-tauri/gen/android
gradle_wrapper=./gradlew
if grep -q "$(printf '\r')" "$gradle_wrapper"; then
  gradle_wrapper=./.gradlew.docker
  tr -d '\r' < ./gradlew > "$gradle_wrapper"
  chmod +x "$gradle_wrapper"
  trap 'rm -f "$gradle_wrapper"' EXIT
fi
bash "$gradle_wrapper" :app:testUniversalDebugUnitTest --no-daemon

echo "Android checks passed."
