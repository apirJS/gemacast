#!/usr/bin/env bash
# Full .deb installer test: install → verify → smoke test → uninstall → verify cleanup.
# Expects env vars: VERSION, ARCH
set -euo pipefail

VERSION="${VERSION:?VERSION is required}"
ARCH="${ARCH:?ARCH is required}"

DEB_FILE="gemacast-pc_${VERSION}_${ARCH}.deb"

if [ ! -f "$DEB_FILE" ]; then
  echo "::error::$DEB_FILE not found"
  exit 1
fi

# ── Install ────────────────────────────────────────────────────────────────
echo "=== Installing .deb ==="
sudo dpkg -i "$DEB_FILE"

# ── Verify Installation ───────────────────────────────────────────────────
echo ""
echo "=== Verifying .deb Installation ==="
ERRORS=0

# Binary installed
if command -v gemacast-pc &>/dev/null; then
  echo "PASS: gemacast-pc is on PATH at $(which gemacast-pc)"
else
  echo "FAIL: gemacast-pc not found on PATH"; ERRORS=$((ERRORS+1))
fi

# ADB installed
if [ -x /usr/lib/gemacast/adb ]; then
  echo "PASS: /usr/lib/gemacast/adb exists and is executable"
else
  echo "FAIL: /usr/lib/gemacast/adb missing or not executable"; ERRORS=$((ERRORS+1))
fi

# Desktop entry
if [ -f /usr/share/applications/gemacast-pc.desktop ]; then
  echo "PASS: Desktop entry installed"
else
  echo "FAIL: Desktop entry not found"; ERRORS=$((ERRORS+1))
fi

# Icon
if [ -f /usr/share/icons/hicolor/256x256/apps/gemacast-pc.png ]; then
  echo "PASS: Application icon installed"
else
  echo "FAIL: Application icon not found"; ERRORS=$((ERRORS+1))
fi

# firewalld service definition
if [ -f /usr/lib/firewalld/services/gemacast.xml ]; then
  echo "PASS: firewalld service definition installed"
else
  echo "FAIL: firewalld service definition not found"; ERRORS=$((ERRORS+1))
fi

# Maintainer scripts present in the package (they run during install, so they are
# not left on disk — inspect the .deb's control archive instead).
CTRL_FILES=$(dpkg-deb --ctrl-tarfile "$DEB_FILE" | tar -tf - 2>/dev/null || true)
for script in postinst postrm; do
  if echo "$CTRL_FILES" | grep -qE "(^|/)${script}\$"; then
    echo "PASS: DEBIAN/${script} present in package"
  else
    echo "FAIL: DEBIAN/${script} missing from package"; ERRORS=$((ERRORS+1))
  fi
done

# Smoke test: run with xvfb (provides virtual display for the tray app)
echo ""
echo "Smoke test: starting gemacast-pc under xvfb..."
xvfb-run --auto-servernum gemacast-pc &
PID=$!
sleep 5

if kill -0 $PID 2>/dev/null; then
  echo "PASS: gemacast-pc is running (PID $PID)"
  kill $PID 2>/dev/null || true
  wait $PID 2>/dev/null || true
else
  echo "FAIL: gemacast-pc exited prematurely"; ERRORS=$((ERRORS+1))
fi

if [ $ERRORS -gt 0 ]; then
  echo ""
  echo "=== $ERRORS installation check(s) failed ==="
  exit 1
fi
echo ""
echo "All .deb installation checks passed"

# ── Uninstall ──────────────────────────────────────────────────────────────
echo ""
echo "=== Uninstalling .deb ==="
sudo dpkg -r gemacast-pc

# ── Verify Uninstall ──────────────────────────────────────────────────────
echo ""
echo "=== Verifying .deb Uninstall ==="
ERRORS=0

if command -v gemacast-pc &>/dev/null; then
  echo "FAIL: gemacast-pc still on PATH after uninstall"; ERRORS=$((ERRORS+1))
else
  echo "PASS: gemacast-pc removed from PATH"
fi

if [ -f /usr/lib/gemacast/adb ]; then
  echo "FAIL: /usr/lib/gemacast/adb still exists"; ERRORS=$((ERRORS+1))
else
  echo "PASS: ADB binary removed"
fi

if [ -f /usr/share/applications/gemacast-pc.desktop ]; then
  echo "FAIL: Desktop entry still exists"; ERRORS=$((ERRORS+1))
else
  echo "PASS: Desktop entry removed"
fi

if [ -f /usr/lib/firewalld/services/gemacast.xml ]; then
  echo "FAIL: firewalld service definition still exists"; ERRORS=$((ERRORS+1))
else
  echo "PASS: firewalld service definition removed"
fi

if [ $ERRORS -gt 0 ]; then
  echo "=== $ERRORS uninstall check(s) failed ==="
  exit 1
fi
echo "All .deb uninstall checks passed"
