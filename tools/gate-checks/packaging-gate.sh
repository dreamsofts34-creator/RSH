#!/usr/bin/env bash
# RSH packaging gate (local-first release rule, user order 2026-11-26 ~16:05 PKT).
#
# v2.0.13 failed FIVE release attempts on packaging while the app code
# built green every time (SHA256SUMS $names, EstimatedSize $installedKb,
# .onInit fall-through, /SD syntax, WordFind minor) — because the local
# gate never compiled the installer or checked the release workflow.
# This step catches that class locally, fast, before any release is
# triggered:
#   1. makensis compiles the REAL installer/rsh.nsi (dummy binaries,
#      real icons, ${VERSION} from the workspace version, the same
#      -D defines the release workflow passes). Missing makensis
#      FAILS LOUDLY — never silently skips.
#   2. .github/workflows/release.yml parses as YAML.
#   3. The headless smoke-test run block stays under GitHub's 21,000
#      char expression limit (indent-stripped, as GitHub counts it).
#   4. Every file the installer references exists in the tree.
#   5. FH4 wheel fix: the installer writes the complete GameInput
#      RacingWheel tree and its vJoyConfig argv matches
#      vjoy_config_argv() (single source of truth — no drift).
# Windows-only runtime behaviour (the headless smoke test itself:
# install/update/uninstall on a real Windows runner) is NOT covered
# here — per the local-first rule it is covered by waiting for the
# fix commit's CI to go fully green before triggering a release.
#
# Usage: tools/gate-checks/packaging-gate.sh [--out DIR]
set -u

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
OUT=""

while [ $# -gt 0 ]; do
    case "$1" in
        --out) OUT="$2"; shift 2 ;;
        *) echo "unknown argument: $1" >&2; exit 2 ;;
    esac
done

fail() { echo "ERROR: $*" >&2; exit 1; }

cd "$ROOT" || exit 2

VER=$(sed -n '/^\[workspace\.package\]/,/^\[/{s/^version = "\(.*\)"/\1/p}' Cargo.toml | head -n 1)
[ -n "$VER" ] || fail "could not read [workspace.package] version from Cargo.toml"
MAJOR="${VER%%.*}"
REST="${VER#*.}"
MINOR="${REST%%.*}"
echo "packaging gate: workspace version $VER (major $MAJOR, minor $MINOR)"

# 1. makensis compiles the real installer script.
command -v makensis >/dev/null 2>&1 || fail "makensis not found — install NSIS (sudo apt-get install -y nsis; CI pins 3.13, local 3.09+ catches script syntax). Refusing to skip."
echo "makensis: $(makensis -VERSION 2>&1 | head -n 1)"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
mkdir -p "$TMP/installer" "$TMP/target/release" "$TMP/src-tauri/binaries" \
    "$TMP/src-tauri/icons" "$TMP/tools/rsh-hm-sidecar" \
    "$TMP/firmware/.pio/build/esp32dev" "$TMP/release-staging"
for f in target/release/rsh-daemon.exe target/release/rsh-ui.exe \
    src-tauri/binaries/RshCoreHiDcr-x86_64-pc-windows-msvc.exe \
    src-tauri/binaries/HIDMaestro.Core.dll \
    tools/rsh-hm-sidecar/HIDMaestro-LICENSE.txt \
    firmware/.pio/build/esp32dev/firmware-signed.bin; do
    echo "dummy" > "$TMP/$f"
done
echo "dummy license" > "$TMP/LICENSE"
for f in icon.ico header.bmp sidebar.bmp; do
    [ -f "src-tauri/icons/$f" ] || fail "installer icon missing: src-tauri/icons/$f"
    cp "src-tauri/icons/$f" "$TMP/src-tauri/icons/$f"
done
cp installer/rsh.nsi "$TMP/installer/rsh.nsi"
python3 - "$TMP/installer/rsh.nsi" "$VER" <<'PYEOF' || fail "version substitution failed"
import re, sys
p, ver = sys.argv[1], sys.argv[2]
s = open(p).read()
s2 = re.sub(r'\$\{VERSION\}', ver, s)
assert '${VERSION}' not in s2, "placeholder remains"
open(p, 'w').write(s2)
PYEOF
(cd "$TMP/installer" && makensis -DESTIMATEDSIZEKB=1 "-DVERSIONMAJOR=$MAJOR" "-DVERSIONMINOR=$MINOR" rsh.nsi >"$TMP/makensis.log" 2>&1) || {
    tail -20 "$TMP/makensis.log" >&2
    fail "makensis failed on installer/rsh.nsi (see above; full log kept in gate OUT dir)"
}
[ -f "$TMP/release-staging/RSH_${VER}_x64-setup.exe" ] || fail "makensis succeeded but no installer was produced"
echo "makensis OK: RSH_${VER}_x64-setup.exe ($(stat -c%s "$TMP/release-staging/RSH_${VER}_x64-setup.exe") B)"
[ -n "$OUT" ] && cp "$TMP/makensis.log" "$OUT/packaging-makensis.log" 2>/dev/null || true

# 2. Release workflow parses as YAML.
python3 -c "import yaml; yaml.safe_load(open('.github/workflows/release.yml'))" 2>&1 \
    || fail "release.yml does not parse as YAML (needs python3 PyYAML)"

# 3. Smoke-test run block under GitHub's 21,000-char expression limit.
python3 - <<'PYEOF' || fail "smoke-test run block is over GitHub's 21,000-char limit"
lines = open('.github/workflows/release.yml').read().splitlines(keepends=True)
name_i = next(i for i, l in enumerate(lines) if l.startswith('      - name: Headless launch smoke test'))
run_i = next(i for i in range(name_i, len(lines)) if lines[i].strip() == 'run: |')
next_step = next(i for i in range(run_i + 1, len(lines)) if lines[i].startswith('      - name:'))
stripped = ''.join(l[8:] if l.startswith('        ') else l for l in lines[run_i + 1:next_step])
print(f"smoke-test run block: {len(stripped)} chars (limit 21000, margin {21000 - len(stripped)})")
assert len(stripped) < 21000, "smoke-test run block over 21000 chars"
PYEOF

# 4. Every file the installer references exists in the tree.
for f in LICENSE target/release/rsh-daemon.exe target/release/rsh-ui.exe \
    src-tauri/binaries/RshCoreHiDcr-x86_64-pc-windows-msvc.exe \
    src-tauri/binaries/HIDMaestro.Core.dll \
    tools/rsh-hm-sidecar/HIDMaestro-LICENSE.txt \
    firmware/.pio/build/esp32dev/firmware-signed.bin \
    src-tauri/icons/icon.ico src-tauri/icons/header.bmp src-tauri/icons/sidebar.bmp; do
    [ -f "$f" ] || echo "NOTE: installer input missing (release build produces it): $f"
done

# 5. FH4 wheel fix (v2.0.14): the installer writes the COMPLETE
# GameInput RacingWheel tree (not just the required subkeys —
# v2.0.13's incomplete tree is the #1 suspect for FH4 ignoring
# the mapping), AND its vJoyConfig argv matches vjoy_config_argv()
# (single source of truth — no drift). Deep NSIS<->Rust equality
# is also covered by the Rust test
# `installer_writes_the_complete_mapping` (rsh-output); this step
# is the fast gate-side tripwire.
python3 - <<'PYEOF' || fail "installer GameInput mapping / vJoyConfig argv check failed"
import re
nsi = open('installer/rsh.nsi').read()
for call in ("Call WriteGameInputVJoyMapping", "Call WriteGameInputHidMapping",
             "Call ConfigureVJoyDevice1"):
    assert call in nsi, f"installer missing {call}"
# Complete-tree markers (both devices): Labels, DPad, Button1,
# Clutch — a required-subkeys-only mapping fails here.
for marker in (r"Labels\Buttons", r"Labels\Switches", r"RacingWheel\DPadUp",
               r"RacingWheel\Button1", r"RacingWheel\Clutch"):
    assert marker in nsi, f"installer mapping missing {marker}"
# vJoyConfig argv: parse the Rust constants, rebuild the argv the
# way vjoy_config_argv() does, and compare with the NSIS call.
rs = open('crates/rsh-output/src/vjoy_config.rs').read()
axes = re.search(r'VJOY_FH4_AXES[^=]*=\s*\[([^\]]*)\]', rs).group(1)
axes = [a.strip().strip('"') for a in axes.split(',') if a.strip()]
buttons = re.search(r'VJOY_FH4_BUTTONS[^=]*=\s*(\d+)', rs).group(1)
povs = re.search(r'VJOY_FH4_DISCRETE_POVS[^=]*=\s*(\d+)', rs).group(1)
ffb = re.search(r'VJOY_FH4_FFB_EFFECTS[^=]*=\s*"([^"]*)"', rs).group(1)
device = re.search(r'VJOY_FH4_DEVICE_ID[^=]*=\s*(\d+)', rs).group(1)
expected = f"{device} -f -a {' '.join(axes)} -b {buttons} -s {povs} -e {ffb}"
m = re.search(r'nsExec::ExecToStack \'"\$R0" ([^\']*)\'', nsi)
assert m, "installer vJoyConfig call not found"
assert m.group(1) == expected, (
    f"installer vJoyConfig argv drifted: NSIS has {m.group(1)!r}, "
    f"vjoy_config_argv() is {expected!r}")
print(f"GameInput mapping complete in installer; vJoyConfig argv matches ({expected})")
PYEOF
echo "packaging gate: PASS"
