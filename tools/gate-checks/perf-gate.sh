#!/usr/bin/env bash
# RSH perf gate — standalone runner (Round 3 WS-2, R3-03).
#
# Enforces the release-binary size + daemon-startup thresholds on the
# RELEASE binaries:
#   * rsh-daemon size <= 27 MB, rsh-ui size <= 34 MB
#   * `rsh-daemon --check` wall time <= 5000 ms
# Record-only (numbers printed, never fail): daemon peak RSS and a
# headless UI launch probe (RSS needs python3 `resource` on Linux;
# the UI probe needs Xvfb; first-paint timing headless (llvmpipe)
# is not comparable to a Windows GPU run).
#
# Thresholds are the measured post-[profile.release] sizes x 1.25,
# rounded up to whole MB (perf-infra report, 2026-11-19: daemon
# 22,041,536 B = 21.02 MB -> 27 MB; UI 27,879,904 B = 26.59 MB ->
# 34 MB; daemon --check 7 ms warm / 32 ms cold, peak RSS 17,748 KB);
# the --check bound is a tripwire, not a tuned number (Round-1 warm
# --check was 7.8 ms, first-ever 214 ms — 5000 ms leaves an order of
# magnitude of headroom). Do NOT change the thresholds here — they
# live in exactly this one place.
#
# Callers:
#   * tools/gate-checks/run-gate.sh — with --record-only-if-missing
#     (the local gate builds debug binaries only, so the step is
#     record-only there and passes with a note).
#   * CI `perf-gate` job (.github/workflows/ci.yml) — fail-closed:
#     release binaries are built first, so a missing binary fails.
#   * release.yml, right after `cargo build --release` — fail-closed.
#
# Usage:
#   tools/gate-checks/perf-gate.sh [--out DIR] [--record-only-if-missing]
#
# Exit code is non-zero when an enforcing check fails (or when the
# release binaries are absent and --record-only-if-missing was not
# given). Every missing tool/binary degrades to record-only with a
# note — the step never fails for a missing python3 or a missing
# Xvfb. On Windows (release.yml, git bash) the `.exe` binaries are
# used; the Xvfb probe is skipped there.
set -u

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
OUT=""
RECORD_ONLY_IF_MISSING=0

while [ $# -gt 0 ]; do
    case "$1" in
        --out) OUT="$2"; shift 2 ;;
        --record-only-if-missing) RECORD_ONLY_IF_MISSING=1; shift ;;
        -h|--help)
            sed -n '2,30p' "$0"
            exit 0
            ;;
        *) echo "unknown argument: $1" >&2; exit 2 ;;
    esac
done

if [ -z "$OUT" ]; then
    OUT="$(mktemp -d /tmp/perf-gate-XXXXXX)"
fi
mkdir -p "$OUT"

daemon_bin="$ROOT/target/release/rsh-daemon"
ui_bin="$ROOT/target/release/rsh-ui"
# Windows (release.yml under git bash): the binaries are .exe.
if [ ! -x "$daemon_bin" ] && [ -x "$daemon_bin.exe" ]; then daemon_bin="$daemon_bin.exe"; fi
if [ ! -x "$ui_bin" ] && [ -x "$ui_bin.exe" ]; then ui_bin="$ui_bin.exe"; fi

daemon_size_max_mb=27
ui_size_max_mb=34
daemon_check_max_ms=5000

if [ ! -x "$daemon_bin" ] || [ ! -x "$ui_bin" ]; then
    if [ "$RECORD_ONLY_IF_MISSING" -eq 1 ]; then
        echo "perf-gate: record-only — release binaries not present"
        echo "  (run 'cargo build --release -p rsh-daemon -p rsh-ui' first;"
        echo "   the gate itself builds debug binaries only)"
        exit 0
    fi
    echo "ERROR: perf-gate: release binaries not present ($daemon_bin / $ui_bin) — build them first ('cargo build --release -p rsh-daemon -p rsh-ui'). Failing closed (R3-03)." >&2
    exit 1
fi

# Binary sizes (enforcing).
daemon_bytes=$(wc -c <"$daemon_bin" | tr -d ' ')
ui_bytes=$(wc -c <"$ui_bin" | tr -d ' ')
daemon_mb=$(( (daemon_bytes + 1048575) / 1048576 ))
ui_mb=$(( (ui_bytes + 1048575) / 1048576 ))
echo "perf-gate: rsh-daemon $daemon_bytes B (${daemon_mb} MB, max ${daemon_size_max_mb} MB)"
echo "perf-gate: rsh-ui $ui_bytes B (${ui_mb} MB, max ${ui_size_max_mb} MB)"
if [ "$daemon_mb" -gt "$daemon_size_max_mb" ]; then
    echo "ERROR: rsh-daemon ${daemon_mb} MB exceeds ${daemon_size_max_mb} MB" >&2
    exit 1
fi
if [ "$ui_mb" -gt "$ui_size_max_mb" ]; then
    echo "ERROR: rsh-ui ${ui_mb} MB exceeds ${ui_size_max_mb} MB" >&2
    exit 1
fi

# Daemon startup: --check starts the runtime + IPC, verifies
# a ping round-trip, and exits 0 (enforcing: wall time).
start_ms=$(date +%s%N | cut -c1-13)
if ! "$daemon_bin" --check >"$OUT/perf-daemon-check.log" 2>&1; then
    echo "ERROR: rsh-daemon --check failed (see $OUT/perf-daemon-check.log)" >&2
    exit 1
fi
end_ms=$(date +%s%N | cut -c1-13)
check_ms=$((end_ms - start_ms))
echo "perf-gate: rsh-daemon --check ${check_ms} ms (max ${daemon_check_max_ms} ms)"
if [ "$check_ms" -gt "$daemon_check_max_ms" ]; then
    echo "ERROR: rsh-daemon --check ${check_ms} ms exceeds ${daemon_check_max_ms} ms" >&2
    exit 1
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
exit 0
