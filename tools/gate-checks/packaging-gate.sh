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
#   5. v2.0.19: the GameInput RacingWheel mappings are GONE — the
#      installer must not define or call WriteGameInputVJoyMapping /
#      WriteGameInputHidMapping, and no GameInput tree markers may
#      remain; the vJoy device-1 envelope (ConfigureVJoyDevice1)
#      stays, and its vJoyConfig argv must match vjoy_config_argv()
#      (single source of truth — no drift).
#   6. v2.0.19: the Steam FH4 OEMData flag is GONE — the installer
#      must never WRITE an OEMData value (no WriteRegBin "OEMData",
#      no SteamFh4OemData marker anywhere); instead the installer
#      deletes the stale OEMData value (HKLM, install time) and the
#      uninstaller deletes it from BOTH hives unconditionally, and
#      removes the two stale GameInput device keys. Only the VALUE
#      is ever deleted, never the OEM key (it is vJoy's).
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
    "$TMP/src-tauri/icons" "$TMP/tools/vigem" \
    "$TMP/firmware/.pio/build/esp32dev" "$TMP/release-staging"
for f in target/release/rsh-daemon.exe target/release/rsh-ui.exe \
    tools/vigem/ViGEmClient.dll \
    tools/vigem/ViGEmBus_1.22.0_x64_x86_arm64.exe \
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
    tools/vigem/ViGEmClient.dll \
    tools/vigem/ViGEmBus_1.22.0_x64_x86_arm64.exe \
    firmware/.pio/build/esp32dev/firmware-signed.bin \
    src-tauri/icons/icon.ico src-tauri/icons/header.bmp src-tauri/icons/sidebar.bmp; do
    [ -f "$f" ] || echo "NOTE: installer input missing (release build produces it): $f"
done

# 5. v2.0.19: the GameInput RacingWheel mappings are GONE (the
# wheel-tab route they served did not work on the user's Microsoft
# Store build of FH4) — the installer must not define or call them,
# and no GameInput tree markers may remain. The vJoy device-1
# envelope (ConfigureVJoyDevice1) stays, and its vJoyConfig argv
# must match vjoy_config_argv() (single source of truth — no drift).
# The envelope match is also covered by the Rust test
# `installer_writes_the_complete_mapping` (rsh-output); this step
# is the fast gate-side tripwire.
python3 - <<'PYEOF' || fail "installer GameInput mapping / vJoyConfig argv check failed"
import re
nsi = open('installer/rsh.nsi').read()
for gone in ("WriteGameInputVJoyMapping", "WriteGameInputHidMapping",
             "Call WriteGameInputVJoyMapping", "Call WriteGameInputHidMapping",
             r"Labels\Buttons", r"Labels\Switches", r"RacingWheel\DPadUp",
             r"RacingWheel\Button1", r"RacingWheel\Clutch"):
    assert gone not in nsi, f"installer still has deleted GameInput mapping: {gone}"
assert "Call ConfigureVJoyDevice1" in nsi, "installer missing Call ConfigureVJoyDevice1"
# vJoyConfig argv: parse the Rust constants, rebuild the argv the
# way vjoy_config_argv() does, and compare with the NSIS call.
rs = open('crates/rsh-output/src/vjoy_config.rs').read()
axes = re.search(r'VJOY_ENVELOPE_AXES[^=]*=\s*\[([^\]]*)\]', rs).group(1)
axes = [a.strip().strip('"') for a in axes.split(',') if a.strip()]
buttons = re.search(r'VJOY_ENVELOPE_BUTTONS[^=]*=\s*(\d+)', rs).group(1)
povs = re.search(r'VJOY_ENVELOPE_DISCRETE_POVS[^=]*=\s*(\d+)', rs).group(1)
ffb = re.search(r'VJOY_ENVELOPE_FFB_EFFECTS[^=]*=\s*"([^"]*)"', rs).group(1)
device = re.search(r'VJOY_DEVICE_ID[^=]*=\s*(\d+)', rs).group(1)
expected = f"{device} -f -a {' '.join(axes)} -b {buttons} -s {povs} -e {ffb}"
m = re.search(r'nsExec::ExecToStack \'"\$R0" ([^\']*)\'', nsi)
assert m, "installer vJoyConfig call not found"
assert m.group(1) == expected, (
    f"installer vJoyConfig argv drifted: NSIS has {m.group(1)!r}, "
    f"vjoy_config_argv() is {expected!r}")
print(f"GameInput mappings gone; vJoyConfig argv matches ({expected})")
PYEOF

# 6. v2.0.19: the Steam FH4 OEMData flag is GONE — the installer
# must never write an OEMData value and no SteamFh4OemData marker
# may remain anywhere. Instead the installer deletes the stale
# OEMData value the old mode left on the user's PC (HKLM, at
# install time) and the uninstaller deletes it from BOTH hives
# unconditionally, plus the two stale GameInput device keys.
# Only the VALUE is ever deleted, never the OEM key (it is vJoy's).
python3 - <<'PYEOF' || fail "installer Steam FH4 OEMData check failed"
import re
nsi = open('installer/rsh.nsi').read()
nsi_lines = nsi.splitlines()
# No OEMData writes anywhere, no marker anywhere.
assert 'steamfh4oemdata' not in nsi.lower(), (
    "installer still references the SteamFh4OemData marker (deleted in v2.0.19)")
for n, raw in enumerate(nsi_lines, 1):
    low = raw.strip().lower()
    if re.search(r'WriteRegBin\b', low) and '"oemdata"' in low:
        raise AssertionError(f"line {n}: installer still WRITES an OEMData value")
    if re.search(r'DeleteRegKey\b', low) and 'joystick\\oem' in low:
        raise AssertionError(
            f"line {n}: DeleteRegKey on a Joystick OEM key — only "
            "the OEMData VALUE may ever be deleted, never the OEM "
            "key (it is vJoy's)")
# Stale-value cleanup: install-time HKLM delete + uninstall deletes
# for BOTH hives, unconditional (no marker gating anymore).
oem_deletes = []
for n, raw in enumerate(nsi_lines, 1):
    if re.search(r'DeleteRegValue\b', raw, re.I) and '"oemdata"' in raw.lower():
        assert 'vid_1234&pid_bead' in raw.lower(), (
            f"line {n}: OEMData delete is not on the vJoy OEM key")
        hive = re.match(r'\s*DeleteRegValue\s+(HKLM|HKCU)\b', raw, re.I).group(1).upper()
        oem_deletes.append((n, hive))
assert oem_deletes, "installer never deletes the stale OEMData value"
assert {h for (_, h) in oem_deletes} == {'HKLM', 'HKCU'}, (
    "stale OEMData cleanup must cover BOTH hives "
    f"(found {sorted({h for (_, h) in oem_deletes})})")
# Uninstaller still removes the two stale GameInput device keys.
for key in ("1234BEAD00010004", "1209525300010004"):
    assert re.search(
        rf'DeleteRegKey HKLM "SYSTEM\\CurrentControlSet\\Control\\'
        rf'GameInput\\Devices\\{key}"', nsi), (
        f"uninstaller no longer removes the GameInput mapping for "
        f"{key}")
print(f"OEMData: no writes, no marker; stale-value deletes at "
      f"lines {[d[0] for d in oem_deletes]}; GameInput keys removed")
PYEOF

echo "packaging gate: PASS"
