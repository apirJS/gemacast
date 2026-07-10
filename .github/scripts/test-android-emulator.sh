#!/usr/bin/env bash
# Android emulator test: install → launch → verify process → uninstall → verify cleanup.
# Expects env var: APK_PATH (path to the signed APK)
# Must be run inside the android-emulator-runner action's script context.
set -euo pipefail

APK_PATH="${APK_PATH:?APK_PATH is required}"

echo "=== Emulator booted ==="

# Install the APK (debug-signed, --no-streaming to avoid Broken pipe on CI emulators)
adb install -t --no-streaming "$APK_PATH"
echo "PASS: APK installed successfully"

# Launch the main activity
adb shell am start -n com.apir.gemacast/.MainActivity
echo "Activity started"

# Wait for the app to initialize
sleep 10

# Verify the app process is running
PID=$(adb shell pidof com.apir.gemacast || true)
if [ -n "$PID" ]; then
  echo "PASS: App is running (PID $PID)"
else
  echo "FAIL: App process not found"
  adb logcat -d -t 50 '*:E' 2>/dev/null | grep -i "gemacast\|fatal\|crash" || true
  exit 1
fi

# Verify activity is in the task stack
ACTIVITY_INFO=$(adb shell dumpsys activity activities 2>/dev/null | grep "com.apir.gemacast" | head -5)
if [ -n "$ACTIVITY_INFO" ]; then
  echo "PASS: Activity found in activity stack"
  echo "$ACTIVITY_INFO"
else
  echo "WARN: Activity not found in activity stack (may be expected for background service apps)"
fi

# Uninstall
adb shell pm uninstall com.apir.gemacast
echo "PASS: APK uninstalled"

# Verify uninstall
STILL_INSTALLED=$(adb shell pm list packages 2>/dev/null | grep "com.apir.gemacast" || true)
if [ -z "$STILL_INSTALLED" ]; then
  echo "PASS: Package fully removed"
else
  echo "FAIL: Package still listed after uninstall"
  exit 1
fi

echo ""
echo "All Android APK checks passed"
