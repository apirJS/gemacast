#!/usr/bin/env bash
# Verify RPM package metadata and contents.
# Expects env vars: VERSION, APPIMAGE_ARCH
set -euo pipefail

VERSION="${VERSION:?VERSION is required}"
APPIMAGE_ARCH="${APPIMAGE_ARCH:?APPIMAGE_ARCH is required}"

RPM_FILE="gemacast-pc-${VERSION}.${APPIMAGE_ARCH}.rpm"

if [ ! -f "$RPM_FILE" ]; then
  echo "::error::$RPM_FILE not found"
  exit 1
fi

# Verify RPM is a valid package
rpm -qip "$RPM_FILE"

# List contents and verify key files are present
CONTENTS=$(rpm -qlp "$RPM_FILE")
echo "$CONTENTS"

ERRORS=0
if echo "$CONTENTS" | grep -q "gemacast-pc"; then
  echo "PASS: Binary found in RPM"
else
  echo "FAIL: Binary not found in RPM"; ERRORS=$((ERRORS+1))
fi

if echo "$CONTENTS" | grep -q "adb"; then
  echo "PASS: ADB found in RPM"
else
  echo "FAIL: ADB not found in RPM"; ERRORS=$((ERRORS+1))
fi

if [ $ERRORS -gt 0 ]; then exit 1; fi
echo "All .rpm structure checks passed"
