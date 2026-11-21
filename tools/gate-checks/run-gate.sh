#!/usr/bin/env bash
# RSH gate runner.
#
# Runs the automated gate harness:
#   0. version-check: [workspace.package] version == the main.slint
#      app-version default, and (when HEAD is exactly tagged) the
#      tag == v<version> (DS-01: the 2.0.2 version drift that made
#      the F23 wizard re-offer inert). Cheap, runs first.
#   1. cargo fmt --check (workspace)
#   2. cargo clippy on the gate crates (rsh-rig + rsh-gate-ui, warnings denied)
#   3. cargo test -p rsh-rig (rig unit tests)
#   4. rsh-rig --gate (the `gate-*` contract checks: runtime +
#      source audits + v2.0.3 lifecycle checks, F17 — real
#      daemon/UI under Xvfb). The count is printed by the runner
#      itself at startup (registry + gate-subset counts) — the
#      header used to hardcode "34" while the registry held 33
#      (R3-14). The default gate deliberately covers ONLY the
#      gate-* contract checks; CI's `rig-gate` job runs the FULL
#      quick suite (all registry scenarios) — see --rig-quick
#      below. A green default gate does NOT reproduce CI.
#   5. rsh-gate-ui (11 headless UI checks: slider/wizard/theme/tray/banner
#      pixels + Extras card size / OFF no-input / responsive gaps)
#   6. windows-check: cargo check --workspace --exclude rsh-app
#      --target x86_64-pc-windows-msvc (check-only, no linking) —
#      Windows-only #[cfg(windows)] code never compiles on this
#      Linux box, so a Windows-target check runs here (rsh-app
#      excluded: ring needs MSVC lib.exe; covered by CI tauri-build).
#      release-blocker class (CI run 44105260838: E0308 in
#      port_conflict.rs, Unix-only bind in the fake daemon) slipped
#      through a green local gate. Runs in the DEFAULT path, not
#      only --full, for exactly that reason.
#   7. perf-gate (PERF-RESEARCH §6 item 9): startup-time + RSS
#      gate on the RELEASE binaries — binary sizes vs thresholds,
#      `rsh-daemon --check` wall time vs threshold (enforcing),
#      daemon peak RSS + headless UI launch probe (record-only:
#      RSS needs python3 `resource` on Linux and is skipped
#      elsewhere; UI startup timing headless is not robust enough
#      to enforce). If the release binaries are absent — the gate
#      builds debug binaries only — the whole step is record-only
#      and passes with a note; the release workflow builds the
#      release binaries first, so the step enforces there.
#      Implementation: tools/gate-checks/perf-gate.sh (R3-03);
#      the same script runs fail-closed in CI's `perf-gate` job
#      and in release.yml right after `cargo build --release`.
#      Cost note (R3-13): the gate-* lifecycle checks carry fixed
#      waits — the 300 s respawn observation in
#      `gate-respawn-bound` is load-bearing (the backoff latch
#      lands at ~260 s; breaking early would mistake a 30 s
#      backoff sleep for the latch) and the 300 s lifecycle soak
#      is inside the 5-min soak cap — so a full --gate run takes
#      >= 10 min before any scenario work is counted.
#
# Usage:
#   tools/gate-checks/run-gate.sh [--full] [--rig-quick] [--out DIR]
#
#   --full  also runs the full workspace suite the gate coordinator
#           owns (cargo test --workspace --exclude rsh-app + workspace
#           clippy -D warnings). NOT run by default: the full suite is
#           the gate coordinator's step, and per user order it runs
#           only at phase gates, not on every small fix.
#   --rig-quick
#           also runs the FULL quick suite (`rsh-rig --rig-quick`,
#           every registry scenario at quick durations — the same
#           command CI's `rig-gate` job runs, ~30 min) as an
#           explicit step after the gate-* checks. NOT run by
#           default: same phase-gate-only rule as --full. Without
#           it the local gate does NOT reproduce CI (R3-02).
#   --out   directory for the run logs (default: a timestamped dir
#           under /tmp).
#
# Exit code is non-zero if any step fails. The rig + UI gate steps
# are EXPECTED to fail while fix workers are still landing fixes —
# see tools/rsh-rig/src/gate_checks.rs for the per-check baseline and owners.
set -u

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
FULL=0
RIG_QUICK=0
OUT=""

while [ $# -gt 0 ]; do
    case "$1" in
        --full) FULL=1; shift ;;
        --rig-quick) RIG_QUICK=1; shift ;;
        --out) OUT="$2"; shift 2 ;;
        -h|--help)
            sed -n '53,70p' "$0" | sed 's/^# //;s/^#$//'
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

# DS-01 (deep scan, U-01): the app version IS the release version.
# The workspace version is the single source of truth; the Slint
# default is a second copy that drifted (still "2.0.2" in the v2.0.3
# build), and the release workflow checks tag == workspace version —
# so the local gate checks both copies agree, and, when HEAD is
# exactly tagged, that the tag matches too. `app_version()` itself
# is pinned by the unit test in crates/rsh-api/src/api.rs.
version_check() {
    local ver
    ver=$(sed -n '/^\[workspace\.package\]/,/^\[/{s/^version = "\(.*\)"/\1/p}' Cargo.toml | head -n 1)
    if [ -z "$ver" ]; then
        echo "ERROR: could not read [workspace.package] version from Cargo.toml" >&2
        return 1
    fi
    if ! grep -Fq "app-version: \"$ver\"" crates/rsh-ui/ui/main.slint; then
        echo "ERROR: main.slint app-version default does not match workspace version $ver" >&2
        return 1
    fi
    if ! grep -Fq "app-version: \"RSH $ver\"" crates/rsh-ui/ui/screens/settings.slint; then
        echo "ERROR: settings.slint app-version default does not match workspace version $ver" >&2
        return 1
    fi
    # The api.rs unit test pins app_version() with a literal (DS-01
    # tripwire): check it here so a missed bump fails the local gate,
    # not a 30-min CI run.
    if ! grep -Fq "assert_eq!(app_version(), \"$ver\")" crates/rsh-api/src/api.rs; then
        echo "ERROR: api.rs app_version() test literal does not match workspace version $ver" >&2
        return 1
    fi
    local tag
    tag=$(git describe --tags --exact-match HEAD 2>/dev/null || true)
    if [ -n "$tag" ] && [ "$tag" != "v$ver" ]; then
        echo "ERROR: HEAD is tagged $tag but workspace version is $ver" >&2
        return 1
    fi
    echo "version $ver (main.slint, settings.slint, api.rs match; tag ${tag:-none} OK)"
}

run_step version-check version_check
run_step fmt cargo fmt --all -- --check
run_step clippy-gate cargo clippy -p rsh-rig -p rsh-gate-ui --all-targets -- -D warnings
run_step rig-tests cargo test -q -p rsh-rig
# F17 lifecycle checks launch the real binaries — build them first
# (the rig stages target/debug/rsh-ui, rsh-daemon, rsh-fake-daemon).
run_step build-lifecycle cargo build -p rsh-ui -p rsh-daemon -p rsh-fake-daemon
# Windows-only code never compiles on this Linux box (see header
# step 6): check the Windows target here, in the DEFAULT gate path.
# If the target is missing, install it via rustup; if rustup or the
# target is unavailable the step FAILS LOUDLY — it never silently
# skips, or the next Windows-only break slips through again.
windows_check() {
    if ! rustup target list --installed 2>/dev/null | grep -qx "x86_64-pc-windows-msvc"; then
        echo "Windows target x86_64-pc-windows-msvc not installed — installing via rustup..."
        if ! command -v rustup >/dev/null 2>&1; then
            echo "ERROR: rustup not found and Windows target x86_64-pc-windows-msvc is not installed — cannot check Windows-only code. Install rustup or the target; refusing to skip." >&2
            return 1
        fi
        rustup target add x86_64-pc-windows-msvc || {
            echo "ERROR: rustup target add x86_64-pc-windows-msvc failed — cannot check Windows-only code. Refusing to skip." >&2
            return 1
        }
    fi
    # rsh-app is excluded: it alone pulls `ring` (tauri-plugin-updater
    # -> reqwest -> rustls -> ring), whose build script needs MSVC
    # `lib.exe`, unavailable on Linux. Same exclusion the --full Linux
    # clippy/test steps already use. rsh-app's own Windows compile is
    # covered by CI's tauri-build job on a real Windows runner. Both
    # CI failure classes (E0308 in rsh-transport, E0425 in
    # rsh-fake-daemon) are inside the checked set.
    cargo check --workspace --exclude rsh-app --target x86_64-pc-windows-msvc
    # DS-07 (deep-scan FC-06/CI-01): test targets need --all-targets.
    # Plain `cargo check` compiles lib + bins only, so the test-only
    # unused `PathBuf` import in crates/rsh-ui/src/ipc_api.rs passed
    # this step locally and failed clippy-windows (--all-targets) in
    # CI on run 44120480671. Workspace-wide --all-targets does NOT
    # run on Linux: it pulls criterion (rsh-benchmarks dev-dep) ->
    # alloca, whose build script needs MSVC `lib.exe` (proven:
    # --all-targets exits 101 on `alloca v0.4.0`, while the plain
    # check passes). So the test-target check is scoped to the ship
    # crates plus rsh-transport — the same set CI's clippy-windows
    # -p run covers, plus rsh-transport (CI run 44947397802: a
    # `doc_markdown` lint in its test target passed this step
    # locally and failed both CI clippy jobs; it has no
    # dev-dependencies, so nothing in its test targets needs MSVC
    # `lib.exe`) — and the full workspace --all-targets runs in the
    # CI windows-check job on a real Windows runner, where MSVC exists.
    cargo check -p rsh-daemon -p rsh-ui -p rsh-api -p rsh-ipc -p rsh-transport --all-targets --target x86_64-pc-windows-msvc
    # DS-08 (CI 44170745634): `cargo check` runs no lints, so a
    # Windows-only clippy lint (cast_possible_truncation in
    # rsh-app-core diagnostics.rs) passed this step locally and
    # failed clippy-windows in CI. Run clippy for the Windows
    # target on the same ship crates (+ rsh-app-core) so the class
    # is caught here. rsh-app itself is excluded (ring needs MSVC
    # lib.exe); its Windows clippy runs in CI's clippy-windows job.
    cargo clippy -p rsh-daemon -p rsh-ui -p rsh-api -p rsh-ipc -p rsh-app-core -p rsh-transport --all-targets --target x86_64-pc-windows-msvc -- -D warnings
}
run_step windows-check windows_check
run_step rig-gate cargo run -q -p rsh-rig -- --gate --out "$OUT/rig"
# R3-02: the full quick suite (same command CI's rig-gate runs),
# opt-in only — ~30 min, phase gates only, like --full.
if [ "$RIG_QUICK" -eq 1 ]; then
    run_step rig-quick cargo run -q -p rsh-rig -- --rig-quick --out "$OUT/rig-quick"
fi
run_step ui-gate cargo run -q -p rsh-gate-ui

# PERF-RESEARCH §6 item 9 (perf-infra): startup-time + RSS gate.
# Enforcing (release binaries present): release binary sizes and
# `rsh-daemon --check` wall time. Record-only (numbers printed,
# never fail): daemon peak RSS and a headless UI launch probe.
# Record-only when the release binaries are absent: the gate builds
# debug binaries only (see `build-lifecycle`); the release workflow
# builds release binaries first, so this step enforces there.
# Robustness: every missing tool/binary degrades to record-only
# with a note — the step never fails for a missing binary, a
# missing python3, or a missing Xvfb. Thresholds are the measured
# post-[profile.release] sizes x 1.25, rounded up to whole MB
# (perf-infra report, 2026-11-19: daemon 22,041,536 B = 21.02 MB
# -> 27 MB; UI 27,879,904 B = 26.59 MB -> 34 MB; daemon --check
# 7 ms warm / 32 ms cold, peak RSS 17,748 KB); the --check bound
# is a tripwire, not a tuned number (Round-1 warm --check was
# 7.8 ms, first-ever 214 ms — 5000 ms leaves an order of
# magnitude of headroom).
perf_gate() {
    # Thin wrapper: the enforcing implementation lives in
    # tools/gate-checks/perf-gate.sh (Round 3 WS-2, R3-03) so CI's
    # `perf-gate` job and release.yml run the exact same checks.
    # The local gate builds debug binaries only, so the step stays
    # record-only here (--record-only-if-missing); CI/release build
    # the release binaries first and run the script fail-closed.
    bash "$ROOT/tools/gate-checks/perf-gate.sh" --out "$OUT" --record-only-if-missing
}
run_step perf-gate perf_gate

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
