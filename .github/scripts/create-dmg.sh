#!/usr/bin/env bash
# Create DMG installers for each macOS variant (x64, aarch64, universal).
# Expects: Gemacast-{x64,aarch64,universal}.app directories to exist
set -euo pipefail

for VARIANT in x64 aarch64 universal; do
  DMG="gemacast-pc-${VARIANT}-apple-darwin.dmg"
  create-dmg \
    --volname "Gemacast" \
    --volicon "gemacast-mobile/src-tauri/icons/icon.icns" \
    --window-pos 200 120 \
    --window-size 600 400 \
    --icon-size 100 \
    --icon "Gemacast-${VARIANT}.app" 150 190 \
    --app-drop-link 450 190 \
    --hide-extension "Gemacast-${VARIANT}.app" \
    "$DMG" \
    "Gemacast-${VARIANT}.app" || true
  # create-dmg returns non-zero when code signing is skipped; this is expected for unsigned builds

  # Verify the DMG was actually produced despite the || true
  if [ ! -f "$DMG" ] || [ ! -s "$DMG" ]; then
    echo "::error::$DMG was not produced by create-dmg"
    exit 1
  fi
  echo "Created: $DMG ($(du -h "$DMG" | cut -f1))"
done
