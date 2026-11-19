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
#   4. rsh-rig --gate (34 rig gate checks: runtime + source audits +
#      10 v2.0.3 lifecycle checks, F17 — real daemon/UI under Xvfb)
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
# see tools/rsh-rig/src/gate_checks.rs for the per-check baseline and owners.
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
    # check passes). So the test-target check is scoped to the four
    # ship crates — the same set CI's clippy-windows -p run covers —
    # and the full workspace --all-targets runs in the CI
    # windows-check job on a real Windows runner, where MSVC exists.
    cargo check -p rsh-daemon -p rsh-ui -p rsh-api -p rsh-ipc --all-targets --target x86_64-pc-windows-msvc
    # DS-08 (CI 44170745634): `cargo check` runs no lints, so a
    # Windows-only clippy lint (cast_possible_truncation in
    # rsh-app-core diagnostics.rs) passed this step locally and
    # failed clippy-windows in CI. Run clippy for the Windows
    # target on the same ship crates (+ rsh-app-core) so the class
    # is caught here. rsh-app itself is excluded (ring needs MSVC
    # lib.exe); its Windows clippy runs in CI's clippy-windows job.
    cargo clippy -p rsh-daemon -p rsh-ui -p rsh-api -p rsh-ipc -p rsh-app-core --all-targets --target x86_64-pc-windows-msvc -- -D warnings
}
run_step windows-check windows_check
run_step rig-gate cargo run -q -p rsh-rig -- --gate --out "$OUT/rig"
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
    local daemon_bin="$ROOT/target/release/rsh-daemon"
    local ui_bin="$ROOT/target/release/rsh-ui"
    local daemon_size_max_mb=27
    local ui_size_max_mb=34
    local daemon_check_max_ms=5000

    if [ ! -x "$daemon_bin" ] || [ ! -x "$ui_bin" ]; then
        echo "perf-gate: record-only — release binaries not present"
        echo "  (run 'cargo build --release -p rsh-daemon -p rsh-ui' first;"
        echo "   the gate itself builds debug binaries only)"
        return 0
    fi

    # Binary sizes (enforcing).
    local daemon_bytes ui_bytes daemon_mb ui_mb
    daemon_bytes=$(wc -c <"$daemon_bin" | tr -d ' ')
    ui_bytes=$(wc -c <"$ui_bin" | tr -d ' ')
    daemon_mb=$(( (daemon_bytes + 1048575) / 1048576 ))
    ui_mb=$(( (ui_bytes + 1048575) / 1048576 ))
    echo "perf-gate: rsh-daemon $daemon_bytes B (${daemon_mb} MB, max ${daemon_size_max_mb} MB)"
    echo "perf-gate: rsh-ui $ui_bytes B (${ui_mb} MB, max ${ui_size_max_mb} MB)"
    if [ "$daemon_mb" -gt "$daemon_size_max_mb" ]; then
        echo "ERROR: rsh-daemon ${daemon_mb} MB exceeds ${daemon_size_max_mb} MB" >&2
        return 1
    fi
    if [ "$ui_mb" -gt "$ui_size_max_mb" ]; then
        echo "ERROR: rsh-ui ${ui_mb} MB exceeds ${ui_size_max_mb} MB" >&2
        return 1
    fi

    # Daemon startup: --check starts the runtime + IPC, verifies
    # a ping round-trip, and exits 0 (enforcing: wall time).
    local start_ms end_ms check_ms
    start_ms=$(date +%s%N | cut -c1-13)
    if ! "$daemon_bin" --check >"$OUT/perf-daemon-check.log" 2>&1; then
        echo "ERROR: rsh-daemon --check failed (see $OUT/perf-daemon-check.log)" >&2
        return 1
    fi
    end_ms=$(date +%s%N | cut -c1-13)
    check_ms=$((end_ms - start_ms))
    echo "perf-gate: rsh-daemon --check ${check_ms} ms (max ${daemon_check_max_ms} ms)"
    if [ "$check_ms" -gt "$daemon_check_max_ms" ]; then
        echo "ERROR: rsh-daemon --check ${check_ms} ms exceeds ${daemon_check_max_ms} ms" >&2
        return 1
    fi

    # Daemon peak RSS (record-only): python3 `resource` reports
    # the waited child's ru_maxrss exactly — /usr/bin/time is not
    # installed on this sandbox, and polling /proc would miss a
    # sub-second peak. Skipped (with a note) where python3 or the
    # resource module is unavailable (e.g. Windows CI).
    if command -v python3 >/dev/null 2>&1; then
        python3 - "$daemon_bin" <<'PYEOF' || echo "perf-gate: RSS probe unavailable (record-only, skipped)"
import resource
import subprocess
import sys
proc = subprocess.run([sys.argv[1], "--check"], capture_output=True)
rss = resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss
unit = "KB (Linux)"
print(f"perf-gate: rsh-daemon --check peak RSS {rss} {unit} (record-only)")
PYEOF
    else
        echo "perf-gate: python3 not found — RSS probe skipped (record-only)"
    fi

    # Headless UI launch probe (record-only): under Xvfb the
    # release UI must stay up until the timeout kills it (exit
    # 124); an instant exit with an error is printed but never
    # fails the gate — first-paint timing headless (llvmpipe) is
    # not comparable to a Windows GPU run.
    if command -v xvfb-run >/dev/null 2>&1; then
        local ui_start ui_end ui_ms ui_rc
        ui_start=$(date +%s%N | cut -c1-13)
        # No `set -e` in this script: a non-zero timeout exit
        # code cannot abort the gate; it is captured in ui_rc.
        timeout 20 xvfb-run -a "$ui_bin" >"$OUT/perf-ui-launch.log" 2>&1
        ui_rc=$?
        ui_end=$(date +%s%N | cut -c1-13)
        ui_ms=$((ui_end - ui_start))
        if [ "$ui_rc" -eq 124 ]; then
            echo "perf-gate: rsh-ui stayed up under Xvfb until the 20 s probe timeout (record-only)"
        else
            echo "perf-gate: rsh-ui exited rc=$ui_rc after ${ui_ms} ms (record-only; see $OUT/perf-ui-launch.log)"
        fi
    else
        echo "perf-gate: xvfb-run not found — UI launch probe skipped (record-only)"
    fi
    return 0
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
