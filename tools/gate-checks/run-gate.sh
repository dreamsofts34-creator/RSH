#!/usr/bin/env bash
# RSH v2.0.2 gate runner — docs/V2.0.2-GATE.md.
#
# Runs the automated gate harness:
#   1. cargo fmt --check (workspace)
#   2. cargo clippy on the gate crates (rsh-rig + rsh-gate-ui, warnings denied)
#   3. cargo test -p rsh-rig (rig unit tests)
#   4. rsh-rig --gate (32 rig gate checks: runtime + source audits +
#      10 v2.0.3 lifecycle checks, F17 — real daemon/UI under Xvfb)
#   5. rsh-gate-ui (6 headless UI checks: slider/wizard/theme/tray pixels)
#
# Usage:
#   tools/gate-checks/run-gate.sh [--full] [--out DIR]
#
#   --full  also runs the full workspace suite the gate coordinator
#           owns (cargo test --workspace --exclude rsh-app + workspace
#           clippy -D warnings). NOT run by default: the full suite is
#           the gate coordinator's step, and per user order it runs
#           only at phase gates, not on every small fix.
#   --out   directory for the run logs (default: a timestamped dir
#           under /tmp).
#
# Exit code is non-zero if any step fails. The rig + UI gate steps
# are EXPECTED to fail while fix workers are still landing fixes —
# see docs/V2.0.2-GATE.md for the per-check baseline and owners.
set -u

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
FULL=0
OUT=""

while [ $# -gt 0 ]; do
    case "$1" in
        --full) FULL=1; shift ;;
        --out) OUT="$2"; shift 2 ;;
        -h|--help)
            sed -n '2,20p' "$0"
            exit 0
            ;;
        *) echo "unknown argument: $1" >&2; exit 2 ;;
    esac
done

if [ -z "$OUT" ]; then
    OUT="/tmp/gate-run-$(date +%Y-%m-%d-%H%M%S)"
fi
mkdir -p "$OUT"

# Cargo lives in ~/.cargo/bin on this machine; pick it up when the
# caller does not already have it on PATH.
if ! command -v cargo >/dev/null 2>&1; then
    export PATH="$HOME/.cargo/bin:$PATH"
fi

cd "$ROOT" || exit 2

declare -a FAILED=()
declare -a PASSED=()

run_step() {
    local name="$1"; shift
    local log="$OUT/$name.log"
    echo "== $name (log: $log)"
    if "$@" >"$log" 2>&1; then
        echo "   PASS $name"
        PASSED+=("$name")
    else
        echo "   FAIL $name (see $log)"
        FAILED+=("$name")
    fi
}

run_step fmt cargo fmt --all -- --check
run_step clippy-gate cargo clippy -p rsh-rig -p rsh-gate-ui --all-targets -- -D warnings
run_step rig-tests cargo test -q -p rsh-rig
# F17 lifecycle checks launch the real binaries — build them first
# (the rig stages target/debug/rsh-ui, rsh-daemon, rsh-fake-daemon).
run_step build-lifecycle cargo build -p rsh-ui -p rsh-daemon -p rsh-fake-daemon
run_step rig-gate cargo run -q -p rsh-rig -- --gate --out "$OUT/rig"
run_step ui-gate cargo run -q -p rsh-gate-ui

if [ "$FULL" -eq 1 ]; then
    run_step clippy-workspace cargo clippy --workspace --exclude rsh-app --all-targets -- -D warnings
    run_step test-workspace cargo test --workspace --exclude rsh-app
fi

echo
echo "== gate summary: ${#PASSED[@]} passed, ${#FAILED[@]} failed (logs in $OUT)"
for name in "${PASSED[@]}"; do echo "   PASS $name"; done
for name in "${FAILED[@]}"; do echo "   FAIL $name"; done

if [ "${#FAILED[@]}" -gt 0 ]; then
    exit 1
fi
exit 0
