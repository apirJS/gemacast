#!/usr/bin/env bash
# Create a universal (fat) .app bundle via lipo from x64 and arm64 builds.
# Expects env vars: VERSION
# Expects directories: mac_x64/, mac_arm64/ with extracted binaries
set -euo pipefail

VERSION="${VERSION:?VERSION is required}"

APP="Gemacast-universal.app"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"

lipo -create -output "$APP/Contents/MacOS/gemacast-pc" \
  mac_x64/gemacast-pc mac_arm64/gemacast-pc

# ADB: prefer x64 for Intel compatibility since arm64 would need Rosetta.
# If only arm64 exists, fall back to it (runs via Rosetta on Intel).
if [ -f mac_x64/adb ] && [ -f mac_arm64/adb ]; then
  lipo -create -output "$APP/Contents/MacOS/adb" mac_x64/adb mac_arm64/adb 2>/dev/null \
    || cp mac_x64/adb "$APP/Contents/MacOS/"
elif [ -f mac_x64/adb ]; then
  cp mac_x64/adb "$APP/Contents/MacOS/"
elif [ -f mac_arm64/adb ]; then
  cp mac_arm64/adb "$APP/Contents/MacOS/"
fi
chmod +x "$APP/Contents/MacOS/"*

sed "s/VERSION_PLACEHOLDER/$VERSION/g" macos/Info.plist > "$APP/Contents/Info.plist"
cp gemacast-mobile/src-tauri/icons/icon.icns "$APP/Contents/Resources/AppIcon.icns"

echo "Universal binary architectures:"
lipo -info "$APP/Contents/MacOS/gemacast-pc"
