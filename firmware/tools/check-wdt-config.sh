#!/usr/bin/env bash
# R2-48 (DS-64): host/CI assertion that the SHIPPED firmware build's
# watchdog config is what the code assumes.
#
# Background: the runtime half of DS-64 landed (loop task + sample task
# subscribe via esp_task_wdt_add, feed via esp_task_wdt_reset, state in
# GET_DIAG as RSH_DIAG_WDT_STATE), but nothing asserted the shipped
# build's WDT config. The timeout is the Arduino-core default — the
# plain `esp32dev` build uses precompiled IDF libs, so sdkconfig flags
# in platformio.ini do NOT apply. What CAN be asserted from the shipped
# build (verified 2026-11-18 against espressif32@6.9.0):
#
#   1. Source: rsh.ino calls esp_task_wdt_add for BOTH tasks (loop in
#      setup(), sample task) and esp_task_wdt_reset on BOTH feed paths.
#   2. App object: src/rsh.ino.cpp.o references (U) esp_task_wdt_add +
#      esp_task_wdt_reset — the call sites are really compiled in, not
#      dead code / compiled out.
#   3. Linked ELF: firmware.elf defines (T) esp_task_wdt_add/reset/init.
#   4. Arduino-core sdkconfig: the precompiled IDF libs shipped with
#      framework-arduinoespressif32 were built with
#      tools/sdk/esp32/sdkconfig, which says:
#        CONFIG_ESP_TASK_WDT=y
#        CONFIG_ESP_TASK_WDT_PANIC=y
#        CONFIG_ESP_TASK_WDT_TIMEOUT_S=5
#        CONFIG_ESP_TASK_WDT_CHECK_IDLE_TASK_CPU0=y
#        (CPU1 idle check NOT set)
#      and the binary contains the string
#      "esp_task_wdt_init(CONFIG_ESP_TASK_WDT_TIMEOUT_S, true)", tying
#      the core's init call to that config value. So the shipped build's
#      WDT timeout IS 5 s with panic-on-timeout — as long as the
#      platform stays pinned (check 5 below).
#   5. Platform pin: platformio.ini pins espressif32@6.9.0 — the
#      framework version the values above were verified against. A
#      platform bump must re-verify the sdkconfig values (this script
#      fails if any value differs, so a bump cannot silently change
#      the timeout).
#
# What this does NOT prove (documented, not hidden):
#   - That the WDT actually fires/resets on a real board — that needs
#     a board test (stall a task, watch for reset). The Virtual Test
#     Rig cannot reproduce a hardware WDT reset.
#   - That the running core's timeout equals the sdkconfig file: the
#     file is the one the precompiled libs shipped with, and the
#     binary's init-call string references the same config symbol —
#     strong, but indirect. A rebuilt-core (secure env, IDF source
#     build) has its own sdkconfig and is NOT covered here.
#
# Usage: run AFTER `pio run -e esp32dev` in firmware/ (CI firmware job
# runs this step right after the build). Exits non-zero on any failure.
#   bash firmware/tools/check-wdt-config.sh
# Env overrides (rare): RSH_PIO_CORE_DIR (PlatformIO core dir),
# RSH_EXPECTED_WDT_TIMEOUT_S (default 5).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
FIRMWARE_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
BUILD_DIR="$FIRMWARE_DIR/.pio/build/esp32dev"
INO="$FIRMWARE_DIR/rsh.ino"
PIO_INI="$FIRMWARE_DIR/platformio.ini"
EXPECTED_TIMEOUT="${RSH_EXPECTED_WDT_TIMEOUT_S:-5}"

fail() { echo "WDT-CHECK FAIL: $*" >&2; exit 1; }
ok()   { echo "WDT-CHECK OK: $*"; }

# --- 1. Source: both tasks subscribe, both feed paths exist ----------
test -f "$INO" || fail "rsh.ino not found at $INO"
n_add="$(grep -c 'esp_task_wdt_add(NULL)' "$INO" || true)"
n_reset="$(grep -c 'esp_task_wdt_reset()' "$INO" || true)"
[ "$n_add" -ge 2 ]   || fail "rsh.ino: expected >=2 esp_task_wdt_add(NULL) (loop + sample task), found $n_add"
[ "$n_reset" -ge 2 ] || fail "rsh.ino: expected >=2 esp_task_wdt_reset() (loop + sample feed), found $n_reset"
grep -q 'RSH_DIAG_WDT_STATE' "$INO" || fail "rsh.ino: no RSH_DIAG_WDT_STATE record (WDT state must stay visible in GET_DIAG)"
ok "source: $n_add esp_task_wdt_add + $n_reset esp_task_wdt_reset call sites in rsh.ino"

# --- 2. Platform pin (the sdkconfig values below belong to this) ------
grep -q '^platform = espressif32@6\.9\.0' "$PIO_INI" \
  || fail "platformio.ini: expected pinned 'platform = espressif32@6.9.0' (WDT values verified against it); re-verify sdkconfig before changing"
ok "platform pinned: espressif32@6.9.0"

# --- 3+4. Built artifacts must exist (run after `pio run`) ------------
ELF="$BUILD_DIR/firmware.elf"
OBJ="$BUILD_DIR/src/rsh.ino.cpp.o"
test -f "$ELF" || fail "firmware.elf missing at $ELF — run 'pio run -e esp32dev' in firmware/ first"
test -f "$OBJ" || fail "rsh.ino.cpp.o missing at $OBJ — run 'pio run -e esp32dev' in firmware/ first"

# nm: prefer the xtensa toolchain PlatformIO installed, fall back to PATH.
NM=""
for cand in \
  "${RSH_PIO_CORE_DIR:-}/packages/toolchain-xtensa-esp32/bin/xtensa-esp32-elf-nm" \
  "${PLATFORMIO_CORE_DIR:-}/packages/toolchain-xtensa-esp32/bin/xtensa-esp32-elf-nm" \
  "$HOME/.platformio/packages/toolchain-xtensa-esp32/bin/xtensa-esp32-elf-nm"; do
  if [ -n "$cand" ] && [ -x "$cand" ]; then NM="$cand"; break; fi
done
if [ -z "$NM" ]; then NM="$(command -v xtensa-esp32-elf-nm || true)"; fi
[ -n "$NM" ] || fail "xtensa-esp32-elf-nm not found (expected under ~/.platformio/packages/toolchain-xtensa-esp32/bin/)"

# App object must REFERENCE the WDT calls (U = undefined = call site).
# (nm output goes to temp files first: `nm | grep -q` under `pipefail`
# is a trap — grep -q exits on first match, nm then dies on SIGPIPE
# and pipefail turns the whole pipeline non-zero.)
TMP_OBJ="$(mktemp)"; TMP_ELF="$(mktemp)"; TMP_STR="$(mktemp)"
trap 'rm -f "$TMP_OBJ" "$TMP_ELF" "$TMP_STR"' EXIT
"$NM" "$OBJ" >"$TMP_OBJ" || fail "nm failed on rsh.ino.cpp.o"
"$NM" "$ELF" >"$TMP_ELF" || fail "nm failed on firmware.elf"
grep -q ' U esp_task_wdt_add' "$TMP_OBJ"   || fail "rsh.ino.cpp.o does not reference esp_task_wdt_add — loop/sample subscribe calls missing from the app object"
grep -q ' U esp_task_wdt_reset' "$TMP_OBJ" || fail "rsh.ino.cpp.o does not reference esp_task_wdt_reset — feed calls missing from the app object"
ok "app object references esp_task_wdt_add + esp_task_wdt_reset (call sites compiled in)"

# Linked ELF must DEFINE the WDT functions (T = in .text).
for sym in esp_task_wdt_add esp_task_wdt_reset esp_task_wdt_init; do
  grep -q " T $sym" "$TMP_ELF" || fail "firmware.elf does not define $sym"
done
ok "firmware.elf defines esp_task_wdt_add/reset/init"

# --- 5. Arduino-core sdkconfig: the config the precompiled libs were --
# built with (this is the shipped build's real WDT config). ----------
CORE_DIR="${RSH_PIO_CORE_DIR:-${PLATFORMIO_CORE_DIR:-$HOME/.platformio}}"
SDKCONFIG="$CORE_DIR/packages/framework-arduinoespressif32/tools/sdk/esp32/sdkconfig"
test -f "$SDKCONFIG" || fail "Arduino-core sdkconfig not found at $SDKCONFIG (set RSH_PIO_CORE_DIR if PlatformIO lives elsewhere)"

check_cfg() { # key expected-value
  local got
  got="$(grep -E "^$1=" "$SDKCONFIG" | cut -d= -f2 || true)"
  [ "$got" = "$2" ] || fail "sdkconfig: $1=$got, expected $2 (re-verify WDT assumptions before accepting)"
  echo "  $1=$got"
}
echo "Arduino-core sdkconfig ($SDKCONFIG):"
check_cfg CONFIG_ESP_TASK_WDT y
check_cfg CONFIG_ESP_TASK_WDT_PANIC y
check_cfg CONFIG_ESP_TASK_WDT_TIMEOUT_S "$EXPECTED_TIMEOUT"
check_cfg CONFIG_ESP_TASK_WDT_CHECK_IDLE_TASK_CPU0 y
grep -q '^# CONFIG_ESP_TASK_WDT_CHECK_IDLE_TASK_CPU1 is not set' "$SDKCONFIG" \
  || fail "sdkconfig: expected CPU1 idle-task check NOT set"
echo "  (CPU1 idle-task check not set, as before)"
ok "core sdkconfig: WDT on, panic on timeout, timeout ${EXPECTED_TIMEOUT} s, idle CPU0 checked"

# Binary ties the core's init call to the same config symbol.
strings "$BUILD_DIR/firmware.bin" >"$TMP_STR" || fail "strings failed on firmware.bin"
grep -q 'esp_task_wdt_init(CONFIG_ESP_TASK_WDT_TIMEOUT_S, true)' "$TMP_STR" \
  || fail "firmware.bin lacks the esp_task_wdt_init(CONFIG_ESP_TASK_WDT_TIMEOUT_S, true) string — core init no longer tied to that config"
ok "firmware.bin contains esp_task_wdt_init(CONFIG_ESP_TASK_WDT_TIMEOUT_S, true)"

echo "WDT-CHECK PASSED: shipped build WDT = enabled, panic-on-timeout, ${EXPECTED_TIMEOUT} s, loop + sample tasks subscribed and fed."
echo "NOT proven by this check (see header): WDT actually firing on a real board — needs a board test, not CI."
