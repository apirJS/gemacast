#!/usr/bin/env bash
# Build an AppImage from an extracted cargo-dist binary archive.
# Always bundles ADB regardless of architecture mismatch.
# Expects env vars: VERSION, ARCH, DIST_EXTRACTED (path to extracted binary dir)
set -euo pipefail

VERSION="${VERSION:?VERSION is required}"
ARCH="${ARCH:?ARCH is required}"
DIST_EXTRACTED="${DIST_EXTRACTED:-dist_extracted}"

APPDIR="Gemacast.AppDir"
mkdir -p "$APPDIR/usr/bin"
mkdir -p "$APPDIR/usr/share/applications"
mkdir -p "$APPDIR/usr/share/icons/hicolor/256x256/apps"

cp "${DIST_EXTRACTED}/gemacast-pc" "$APPDIR/usr/bin/"

# Always bundle ADB regardless of architecture mismatch.
# On ARM64 hosts, the x86_64 adb can run via qemu-user-static / binfmt_misc.
# With the build.rs fix, ARM64 builds now download native ARM64 adb.
if [ -f "${DIST_EXTRACTED}/adb" ]; then
  cp "${DIST_EXTRACTED}/adb" "$APPDIR/usr/bin/"
  ADB_ARCH=$(file "${DIST_EXTRACTED}/adb" | grep -oP 'ELF \d+-bit .+?, \K[^,]+' || echo "unknown")
  echo "Bundled adb (binary arch: $ADB_ARCH, target: $ARCH)"
  if ! echo "$ADB_ARCH" | grep -qi "$ARCH\|$([ "$ARCH" = "x86_64" ] && echo 'X86-64' || echo 'AArch64')"; then
    echo "::notice::ADB arch ($ADB_ARCH) differs from target ($ARCH) — requires qemu-user-static or binfmt_misc on ARM64 hosts"
  fi
else
  echo "::warning::ADB binary not found in ${DIST_EXTRACTED}/"
fi
chmod +x "$APPDIR/usr/bin/"*

cp linux/gemacast-pc.desktop "$APPDIR/"
cp linux/gemacast-pc.desktop "$APPDIR/usr/share/applications/"
cp linux/gemacast-pc.png "$APPDIR/gemacast-pc.png"
cp linux/gemacast-pc.png "$APPDIR/usr/share/icons/hicolor/256x256/apps/gemacast-pc.png"
cp linux/gemacast-pc.png "$APPDIR/.DirIcon"

# Create AppRun — written via printf to avoid heredoc/YAML conflicts
printf '#!/bin/bash\nSELF="$(readlink -f "$0")"\nHERE="${SELF%%/*}"\nexport PATH="${HERE}/usr/bin:${PATH}"\nexec "${HERE}/usr/bin/gemacast-pc" "$@"\n' > "$APPDIR/AppRun"
chmod +x "$APPDIR/AppRun"

# Download appimagetool for the current architecture
wget -q "https://github.com/AppImage/AppImageKit/releases/download/continuous/appimagetool-${ARCH}.AppImage" -O appimagetool
chmod +x appimagetool

./appimagetool --appimage-extract-and-run --no-appstream "$APPDIR" \
  "gemacast-pc-${VERSION}-${ARCH}.AppImage"

echo "Built: gemacast-pc-${VERSION}-${ARCH}.AppImage"
