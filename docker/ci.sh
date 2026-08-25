#!/bin/sh
# Run every CI gate that can run on Linux, in ci.yml's order.
#
# Covers 3 of CI's 6 jobs: `audit`, `frontend`, and the backend matrix's
# **Linux PC** leg. The other three cannot run here — Windows PC and macOS PC
# need their own kernels, and the Linux Android leg needs the NDK/SDK/JDK
# toolchain this image deliberately omits (see docker/README.md). A green run
# therefore does NOT mean CI is green; it means the Linux-checkable half is.
#
# Gates run to completion even after one fails, mirroring ci.yml's
# `fail-fast: false` — one run should surface every problem, not just the first.
# Exit status is non-zero if any gate failed.
#
# Invoked as the image's default CMD, so it inherits the live PipeWire +
# WirePlumber session that entrypoint.sh sets up. `cargo test --workspace` needs
# that session for the 7 `#[serial(pipewire)]` tests; the other gates ignore it.
#
#   docker compose -f docker/compose.yaml run --rm test            # all gates
#   docker compose -f docker/compose.yaml run --rm test ci.sh -k frontend
#
# Deliberately does not `set -e`: the whole point is to keep going.

MOBILE_DIR=/work/gemacast-mobile

# Gates to skip, space-separated, from `-k`/`--skip`.
SKIP=""
while [ "$#" -gt 0 ]; do
    case "$1" in
        -k|--skip)
            if [ -z "$2" ]; then
                echo "ci.sh: -k needs a group name" >&2
                exit 2
            fi
            SKIP="$SKIP $2"; shift 2 ;;
        -h|--help)
            echo "usage: ci.sh [-k audit|frontend|backend]..."
            exit 0 ;;
        *) echo "ci.sh: unknown argument '$1'" >&2; exit 2 ;;
    esac
done

# Results accumulate newline-separated, never space-separated: every label
# contains a space ("backend: cargo fmt"), so a `for g in $PASSED` word-split
# would shred each one into three meaningless tokens in the summary.
NL='
'
PASSED=""
SKIPPED=""
FAILED=""

# Marks a gate group as skipped when named in SKIP. Group names are coarse
# (audit/frontend/backend) so `-k frontend` drops all five bun gates at once.
skipping() {
    for s in $SKIP; do
        [ "$s" = "$1" ] && return 0
    done
    return 1
}

# run <group> <label> <command...>
run() {
    group=$1; label=$2; shift 2
    if skipping "$group"; then
        SKIPPED="$SKIPPED$label$NL"
        return
    fi
    printf '\n\033[1;34m=== %s ===\033[0m\n' "$label"
    printf '\033[2m$ %s\033[0m\n' "$*"
    if "$@"; then
        PASSED="$PASSED$label$NL"
    else
        # Capture into a variable immediately: the next printf would otherwise
        # overwrite $? before the summary could record it.
        status=$?
        FAILED="$FAILED$label (exit=$status)$NL"
    fi
}

# report <ansi-code> <tag> <newline-separated-list>
report() {
    code=$1; tag=$2; list=$3
    [ -n "$list" ] || return 0
    printf '%s' "$list" | while IFS= read -r line; do
        [ -n "$line" ] && printf '  \033[%sm%s\033[0m  %s\n' "$code" "$tag" "$line"
    done
}

# -- audit --------------------------------------------------------------------
# `cargo audit` reads Cargo.lock only, so it needs no build and runs first —
# the same reason CI gives it its own fast job.
run audit "audit: cargo audit" cargo audit

# -- frontend -----------------------------------------------------------------
# `bun install` must land in the image, not the mounted tree: the host's
# node_modules holds Windows shims (esbuild.exe, eslint.exe) that cannot execute
# here. compose.yaml mounts a named volume over gemacast-mobile/node_modules for
# exactly this; without it every bun gate below fails on a bad interpreter.
if [ -d "$MOBILE_DIR" ]; then
    cd "$MOBILE_DIR" || exit 1
    run frontend "frontend: bun install" bun install --frozen-lockfile
    run frontend "frontend: prettier"    bun run format:check
    run frontend "frontend: eslint"      bun run lint
    run frontend "frontend: typecheck"   bun run typecheck
    run frontend "frontend: bun test"    bun test
    cd /work || exit 1
else
    echo "ci.sh: $MOBILE_DIR missing — is the repo mounted at /work?" >&2
    FAILED="$FAILED frontend: repo not mounted$NL"
fi

# -- backend: Linux PC leg ----------------------------------------------------
# Order matches ci.yml: fmt, then clippy, then the suite. clippy and test are
# separate cargo invocations against the same CARGO_TARGET_DIR, so the second
# reuses the first's artifacts.
run backend "backend: cargo fmt"    cargo fmt --check
run backend "backend: cargo clippy" cargo clippy --workspace --all-targets -- -D warnings
run backend "backend: cargo test"   cargo test --workspace

# -- summary ------------------------------------------------------------------
printf '\n\033[1m-------- summary --------\033[0m\n'
report 32 PASS "$PASSED"
report 2  SKIP "$SKIPPED"
report 31 FAIL "$FAILED"

if [ -n "$FAILED" ]; then
    printf '\n\033[31mFAILED\033[0m — see above.\n'
    exit 1
fi
printf '\n\033[32mAll Linux-checkable gates passed.\033[0m\n'
printf 'Not covered here: Windows PC, macOS PC, Linux Android.\n'
