#!/bin/sh
# Run commands inside the PipeWire session required by Linux tests.
set -e

export XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-/tmp/runtime-dir}"
mkdir -p "$XDG_RUNTIME_DIR"
chmod 0700 "$XDG_RUNTIME_DIR"
export PIPEWIRE_LOG_LEVEL="${PIPEWIRE_LOG_LEVEL:-2}"

if [ "$#" -eq 0 ]; then
    set -- ci.sh
fi

exec dbus-run-session -- sh -c '
    pipewire 2>/dev/null &
    sleep 2
    wireplumber 2>/dev/null &
    sleep 2
    exec "$@"
' sh "$@"
