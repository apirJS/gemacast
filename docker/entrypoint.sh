#!/bin/sh
# Bring up a private D-Bus + PipeWire + WirePlumber session, then run the
# requested command inside it. Mirrors the CI "Cargo Test (Linux with PipeWire)"
# step; see .github/workflows/ci.yml.
#
# The daemons must be running before `cargo test` starts because the 7
# `#[serial(pipewire)]` tests (Linux capture adapters + process_lister) talk to
# a live daemon. Everything else in the suite is hardware-free by construction
# and does not care.
set -e

# PipeWire refuses to start without a writable XDG_RUNTIME_DIR at mode 0700.
export XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-/tmp/runtime-dir}"
mkdir -p "$XDG_RUNTIME_DIR"
chmod 0700 "$XDG_RUNTIME_DIR"

# Quiet by default (2 = warnings). Override with -e PIPEWIRE_LOG_LEVEL=4 when
# diagnosing a daemon problem rather than a test failure.
export PIPEWIRE_LOG_LEVEL="${PIPEWIRE_LOG_LEVEL:-2}"

# Fall back to the image's CMD if invoked bare.
if [ "$#" -eq 0 ]; then
    set -- ci.sh
fi

# `dbus-run-session` gives the daemons a session bus that dies with this
# process, so nothing leaks between `docker run` invocations. stderr from both
# daemons is dropped: headless containers emit harmless "XOpenDisplay() failed"
# noise that otherwise buries real test output. The sleeps are the same crude
# readiness wait CI uses — PipeWire needs to own the socket before WirePlumber
# connects, and WirePlumber needs to have claimed session management before the
# first test enumerates nodes.
exec dbus-run-session -- sh -c '
    pipewire 2>/dev/null &
    sleep 2
    wireplumber 2>/dev/null &
    sleep 2
    exec "$@"
' sh "$@"
