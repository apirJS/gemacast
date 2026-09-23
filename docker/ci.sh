#!/bin/sh
# Run Linux-supported CI gates while collecting every failure.

MOBILE_DIR=/work/gemacast-mobile
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

NL='
'
PASSED=""
SKIPPED=""
FAILED=""

skipping() {
    for s in $SKIP; do
        [ "$s" = "$1" ] && return 0
    done
    return 1
}

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
        status=$?
        FAILED="$FAILED$label (exit=$status)$NL"
    fi
}

report() {
    code=$1; tag=$2; list=$3
    [ -n "$list" ] || return 0
    printf '%s' "$list" | while IFS= read -r line; do
        [ -n "$line" ] && printf '  \033[%sm%s\033[0m  %s\n' "$code" "$tag" "$line"
    done
}

run audit "audit: cargo audit" cargo audit

if [ -d "$MOBILE_DIR" ]; then
    cd "$MOBILE_DIR" || exit 1
    run frontend "frontend: bun install" bun install --frozen-lockfile
    run frontend "frontend: prettier"    bun run format:check
    run frontend "frontend: eslint"      bun run lint
    run frontend "frontend: typecheck"   bun run typecheck
    run frontend "frontend: bun test"    bun test
    cd /work || exit 1
else
    echo "ci.sh: $MOBILE_DIR missing; mount the repository at /work" >&2
    FAILED="$FAILED frontend: repo not mounted$NL"
fi

run backend "backend: cargo fmt"    cargo fmt --check
run backend "backend: cargo clippy" cargo clippy --workspace --all-targets -- -D warnings
run backend "backend: cargo test"   cargo test --workspace

printf '\n\033[1m-------- summary --------\033[0m\n'
report 32 PASS "$PASSED"
report 2  SKIP "$SKIPPED"
report 31 FAIL "$FAILED"

if [ -n "$FAILED" ]; then
    printf '\n\033[31mFAILED\033[0m; see above.\n'
    exit 1
fi
printf '\n\033[32mAll Linux checks passed.\033[0m\n'
