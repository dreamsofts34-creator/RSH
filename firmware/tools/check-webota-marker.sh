#!/usr/bin/env bash
# F-13 (polish round): host/CI assertion that the SHIPPED firmware build
# contains the WebOTA anti-downgrade version marker.
#
# Background: WebOTA rejects a downgrade by scanning the uploaded image
# for the literal "Version <b>" followed by the version digits
# (ota_validate.h streaming scanner; webota.cpp:174 builds the page chip
# as one concatenated literal so marker + digits sit next to each other
# in the flashed binary). If a future page redesign drops or rewords
# that chip, EVERY OTA upload is rejected ("image mein version nahi
# mila") and the board can never be updated over the air again.
#
# This script fails the build if the BUILT binary lacks
# "Version <b><FIRMWARE_VERSION></b>". It checks:
#   1. Source: webota.cpp still builds the chip from the "Version <b>"
#      literal + FIRMWARE_VERSION (a redesign that splits the literal
#      breaks the adjacency the scanner relies on).
#   2. Version: version.h triple parses to the FIRMWARE_VERSION string.
#   3. Binary: the built firmware.bin (or the merged factory image)
#      contains the exact marker bytes.
#
# What this does NOT prove: that the marker scanner itself accepts a
# real image (host tests cover ota_validate.h scanning), or that an
# attacker cannot forge a marker (they can — the marker is only a
# downgrade hint; the Ed25519 signature check after it is the real
# gate, webota.cpp:335-340).
#
# Usage: run AFTER `pio run -e esp32dev` in firmware/ (CI firmware job
# runs this step right after the WDT config check). Exits non-zero on
# any failure.
#   bash firmware/tools/check-webota-marker.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
FIRMWARE_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
BUILD_DIR="$FIRMWARE_DIR/.pio/build/esp32dev"
WEBOTA="$FIRMWARE_DIR/webota.cpp"
VERSION_H="$FIRMWARE_DIR/version.h"

fail() { echo "WEBOTA-MARKER-CHECK FAIL: $*" >&2; exit 1; }
ok()   { echo "WEBOTA-MARKER-CHECK OK: $*"; }

# --- 1. Source literal intact ------------------------------------------
test -f "$WEBOTA" || fail "webota.cpp not found at $WEBOTA"
grep -q 'Version <b>)HTML" FIRMWARE_VERSION R"HTML(</b>' "$WEBOTA" \
  || fail "webota.cpp: version chip literal changed — expected 'Version <b>)HTML\" FIRMWARE_VERSION R\"HTML(</b>' on one line (marker + version must stay adjacent in the binary; see F-13)"
ok "source: version chip literal intact in webota.cpp"

# --- 2. Version triple from version.h ----------------------------------
major="$(grep -E '^#define RSH_FW_VERSION_MAJOR' "$VERSION_H" | awk '{print $3}')"
minor="$(grep -E '^#define RSH_FW_VERSION_MINOR' "$VERSION_H" | awk '{print $3}')"
patch="$(grep -E '^#define RSH_FW_VERSION_PATCH' "$VERSION_H" | awk '{print $3}')"
[ -n "$major" ] && [ -n "$minor" ] && [ -n "$patch" ] \
  || fail "version.h: could not parse RSH_FW_VERSION_MAJOR/MINOR/PATCH"
VERSION="$major.$minor.$patch"
MARKER="Version <b>$VERSION</b>"
ok "version: $VERSION (from version.h)"

# --- 3. Built binary contains the marker -------------------------------
BIN="$BUILD_DIR/firmware.bin"
if [ ! -f "$BIN" ]; then
  # Fall back to the merged factory image (same app bytes inside).
  shopt -s nullglob
  merged=("$BUILD_DIR"/RSH-Firmware-DevKitV1-v*.bin)
  [ "${#merged[@]}" -gt 0 ] || fail "no built firmware.bin and no merged image in $BUILD_DIR — run 'pio run -e esp32dev' in firmware/ first"
  BIN="${merged[0]}"
fi
grep -a -F -q "$MARKER" "$BIN" \
  || fail "built binary $BIN lacks marker '$MARKER' — WebOTA anti-downgrade would reject every upload (page redesign broke the chip?)"
ok "binary: marker '$MARKER' present in $(basename "$BIN")"
