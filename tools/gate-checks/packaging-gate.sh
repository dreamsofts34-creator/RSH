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
#   6. Steam FH4 OEMData (v2.0.15, Part A3): the OEMData wheel flag
#      is written ONLY from the optional, off-by-default installer
#      section (a silent /S install never installs /o sections,
#      so a default install has no OEMData), only for the vJoy
#      device (never HID — A4), with the exact PC-tested bytes,
#      and only the VALUE (never the OEM key); the uninstaller
#      deletes the VALUE only when the RSH marker is present.
#      The marker is a 64-bit-view value — the 32-bit installer
#      would otherwise read/write WOW6432Node and never see it —
#      so the gate tracks SetRegView for the marker ops (the
#      OEMData key itself lives under SYSTEM\CurrentControlSet,
#      which is NOT redirected, so no view assertion there).
#      The GameInput mapping functions (check 5) must not have
#      grown OEMData writes either.
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

# 6. Steam FH4 OEMData (v2.0.15, Part A3): the flag lives ONLY in
# the optional, off-by-default installer section — a silent /S
# install never installs /o sections, so a default install has
# no OEMData. The section IS the installer checkbox (MUI
# components page); an ${If}-gated write in the main section
# would NOT pass this check — the write must be lexically inside
# the /o section. Marker-gated uninstall delete, value-only.
python3 - <<'PYEOF' || fail "installer Steam FH4 OEMData check failed"
import re
nsi_lines = open('installer/rsh.nsi').read().splitlines()

# --- tiny NSIS context tracker: current section (/o? uninstall?),
# LogicLib ${If} depth, and the last SetRegView (32-bit installer
# defaults to the 32-bit view; only SetRegView 64 reaches the
# native view where the 64-bit daemon reads the marker).
section = None        # (name, optional?)
if_depth = 0
regview = 32
oem_writes = []       # (line_no, section, regview)
marker_writes = []    # (line_no, section, regview)
marker_reads = []     # (line_no, section, regview)
oem_deletes = []      # (line_no, section, if_depth, hive)
marker_deletes = []   # (line_no, section, if_depth)
for n, raw in enumerate(nsi_lines, 1):
    line = raw.strip()
    m = re.match(r'Section\s+(/o\s+)?"([^"]+)"', line, re.I)
    if m:
        section = (m.group(2), bool(m.group(1)))
        continue
    if re.match(r'SectionEnd\b', line, re.I):
        section = None
        continue
    if re.match(r'\$\{(If|IfNot)\b', line):
        if_depth += 1
    elif re.match(r'\$\{EndIf\}', line):
        assert if_depth > 0, f"line {n}: ${{EndIf}} without ${{If}}"
        if_depth -= 1
    m = re.match(r'SetRegView\s+(32|64)\b', line, re.I)
    if m:
        regview = int(m.group(1))
        continue
    low = line.lower()
    # A DeleteRegKey on a Joystick OEM key has no "OEMData" text in
    # it (the key path ends at ...\OEM\VID_...), so this guard lives
    # outside the 'oemdata' branch below — only the VALUE may ever
    # be deleted, never the OEM key (it is vJoy's).
    if re.search(r'DeleteRegKey\b', line, re.I) and 'joystick\\oem' in low:
        assert False, (
            f"line {n}: DeleteRegKey on a Joystick OEM key — only "
            "the OEMData VALUE may ever be deleted, never the OEM "
            "key (it is vJoy's)")
    if 'oemdata' in low:
        assert 'vid_1209&pid_5253' not in low, (
            f"line {n}: OEMData touched on the HID device "
            "(A4: never write it for VID_1209&PID_5253)")
        # The VALUE is spelled "OEMData" (quoted). The marker value
        # "SteamFh4OemData" also contains the substring — it is NOT
        # the flag, so only the quoted exact name counts here; the
        # marker lines are handled by the SteamFh4OemData branch.
        if re.search(r'WriteRegBin\b', line, re.I) and '"oemdata"' in low:
            oem_writes.append((n, section, regview))
            assert 'vid_1234&pid_bead' in low, (
                f"line {n}: OEMData write is not on the vJoy OEM key "
                "(VID_1234&PID_BEAD)")
            assert '4000000000000000' in low, (
                f"line {n}: OEMData bytes are not 40 00 00 00 00 00 00 00")
        elif re.search(r'DeleteRegValue\b', line, re.I) and '"oemdata"' in low:
            hive = re.match(r'DeleteRegValue\s+(HKLM|HKCU)\b', line, re.I).group(1).upper()
            assert 'vid_1234&pid_bead' in low, (
                f"line {n}: OEMData delete is not on the vJoy OEM key")
            oem_deletes.append((n, section, if_depth, hive))
    if 'steamfh4oemdata' in low:
        if re.search(r'WriteReg(DWORD|Str)\b', line, re.I):
            assert re.search(r'HKLM\s+"Software\\RSH"', line, re.I), (
                f"line {n}: SteamFh4OemData marker must live under "
                'HKLM "Software\\RSH"')
            marker_writes.append((n, section, regview))
        elif re.search(r'ReadReg(DWORD|Str)\b', line, re.I):
            marker_reads.append((n, section, regview))
        elif re.search(r'DeleteRegValue\b', line, re.I):
            marker_deletes.append((n, section, if_depth))

# (a) Default install writes no OEMData: exactly one write in the
# whole file, and it sits in the optional /o "Steam FH4" section.
assert len(oem_writes) == 1, (
    f"expected exactly 1 OEMData write in installer/rsh.nsi, found "
    f"{len(oem_writes)}: {[w[0] for w in oem_writes]}")
wline, (wsec, wopt), _ = oem_writes[0]
assert wopt and wsec is not None and re.search(r'FH4', wsec, re.I), (
    f"line {wline}: OEMData write is in section {wsec!r} — it must "
    "be in the optional (/o) Steam FH4 section only, so a default "
    "install never writes it")
assert 'MUI_PAGE_COMPONENTS' in open('installer/rsh.nsi').read(), (
    "installer has no MUI_PAGE_COMPONENTS — the /o section would "
    "have no checkbox page")
# The marker is written next to the flag, same section, native view.
assert len(marker_writes) == 1, (
    f"expected exactly 1 SteamFh4OemData marker write, found "
    f"{len(marker_writes)}: {[w[0] for w in marker_writes]}")
mline, (msec, mopt), mview = marker_writes[0]
assert mopt and msec == wsec, (
    f"line {mline}: marker write is not in the same optional "
    f"Steam FH4 section as the OEMData write (line {wline})")
assert mview == 64, (
    f"line {mline}: marker write is in the {mview}-bit registry "
    "view — it must be SetRegView 64 (HKLM\\Software IS "
    "redirected; the 64-bit daemon would never see a WOW6432Node "
    "marker)")

# (b) Uninstaller deletes the VALUE only when the marker is
# present: marker read first (native view), then If-gated deletes.
un_reads = [r for r in marker_reads if r[1] and r[1][0] == 'Uninstall']
assert un_reads, (
    "uninstaller never reads the SteamFh4OemData marker — it "
    "cannot know whether RSH wrote the flag")
rline, _, rview = un_reads[0]
assert rview == 64, (
    f"line {rline}: marker read is in the {rview}-bit registry "
    "view — must match the write's SetRegView 64")
assert oem_deletes, "uninstaller never deletes the OEMData value"
for dline, (dsec, _), ddepth, hive in oem_deletes:
    assert dsec == 'Uninstall', (
        f"line {dline}: OEMData delete outside the Uninstall section")
    assert ddepth >= 1, (
        f"line {dline}: OEMData delete is NOT inside an ${{If}} — "
        "the uninstaller must delete it only when the marker is present")
    assert dline > rline, (
        f"line {dline}: OEMData delete comes before the marker read "
        f"(line {rline}) — the gate cannot be evaluated yet")
assert {h for (_, _, _, h) in oem_deletes} == {'HKLM', 'HKCU'}, (
    "uninstaller must delete the OEMData value from BOTH hives "
    "(HKLM written by the installer, HKCU by the daemon) — found "
    f"{sorted({h for (_, _, _, h) in oem_deletes})}")
assert marker_deletes, (
    "uninstaller deletes OEMData but never clears the "
    "SteamFh4OemData marker value")
for dline, (dsec, _), ddepth in marker_deletes:
    assert dsec == 'Uninstall' and ddepth >= 1, (
        f"line {dline}: marker delete must be If-gated in the "
        "Uninstall section")

# (c) GameInput mapping functions untouched by the OEMData work:
# still defined, and no OEMData / SteamFh4 reference inside them.
nsi = '\n'.join(nsi_lines)
for fn in ("WriteGameInputVJoyMapping", "WriteGameInputHidMapping"):
    m = re.search(rf'Function {fn}\b(.*?)^FunctionEnd',
                  nsi, re.S | re.M | re.I)
    assert m, f"installer lost Function {fn}"
    body = m.group(1)
    assert 'oemdata' not in body.lower(), (
        f"Function {fn} now references OEMData — the mapping and "
        "the Steam flag must stay separate")
    assert 'steamfh4' not in body.lower(), (
        f"Function {fn} now references SteamFh4")
for key in ("1234BEAD00010004", "1209525300010004"):
    assert re.search(
        rf'DeleteRegKey HKLM "SYSTEM\\CurrentControlSet\\Control\\'
        rf'GameInput\\Devices\\{key}"', nsi), (
        f"uninstaller no longer removes the GameInput mapping for "
        f"{key}")
print(f"OEMData: 1 write in optional section {wsec!r} (line {wline}), "
      f"marker write line {mline}, marker-gated uninstall deletes "
      f"lines {[d[0] for d in oem_deletes]}; mapping functions clean")
PYEOF
echo "packaging gate: PASS"
