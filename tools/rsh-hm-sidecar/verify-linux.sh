#!/usr/bin/env bash
# verify-linux.sh — sandbox check for the production sidecar.
# Builds the sidecar as a plain .NET app (no AOT — cross-OS native
# compilation is not supported) against the REAL HIDMaestro v2.1.0
# DLL, then runs a scripted session and asserts the protocol facts
# that are checkable without Windows: hello, version, describe-profile
# (derived 15-byte report size), install-driver (not-admin path on
# Linux), unknown-command handling.
#
# Usage: ./verify-linux.sh [path/to/HIDMaestro.Core.dll]
# Default DLL path: $HM_DLL or ~/tmp/hm-dll/HIDMaestro.Core.dll
# The DLL is never committed; fetch v2.1.0 and verify its SHA-256
# (f13e32591404db0f418af9265b8e240f71257297cfae5a41dcd696fd87e9b4d5).
set -euo pipefail
cd "$(dirname "$0")"

DLL="${1:-${HM_DLL:-$HOME/tmp/hm-dll/HIDMaestro.Core.dll}}"
DOTNET="${DOTNET:-$HOME/.dotnet/dotnet}"

if [ ! -f "$DLL" ]; then
  echo "DLL not found at $DLL — fetch HIDMaestro v2.1.0 (see README.md)" >&2
  exit 1
fi

"$DOTNET" build -c Release -p:HidMaestroDllPath="$DLL" --nologo -v q

DLL_OUT="bin/Release/net10.0-windows/rsh-hm-sidecar.dll"
if [ ! -f "$DLL_OUT" ]; then
  echo "build output missing: $DLL_OUT" >&2
  exit 1
fi

printf '%s\n' \
  '{"cmd":"ping"}' \
  '{"cmd":"version"}' \
  '{"cmd":"describe-profile"}' \
  '{"cmd":"install-driver"}' \
  '{"cmd":"bogus"}' \
  '{"cmd":"create","profile":"generic-dinput-gamepad"}' \
  '{"cmd":"submit","axes":{"X":0.5},"buttons":0}' \
  '{"cmd":"exit"}' \
  | "$DOTNET" "$DLL_OUT" 2>verify-linux.stderr > verify-linux.stdout

python3 - <<'PYEOF'
import json, sys

lines = [json.loads(l) for l in open("verify-linux.stdout") if l.strip()]
assert lines[0].get("hello") == "rsh-hm-sidecar", f"hello: {lines[0]}"
assert lines[0].get("sdk") == "2.1.0", f"sdk: {lines[0]}"
assert lines[0].get("protocol") == 1, f"protocol: {lines[0]}"
assert lines[0].get("dll_sha256_ok") is True, f"dll hash: {lines[0]}"
assert lines[1].get("ok") is True and lines[1].get("pong") is True, f"ping: {lines[1]}"
assert lines[2].get("sdk") == "2.1.0", f"version: {lines[2]}"
prof = lines[3]
assert prof.get("profile") == "rsh-wheel-custom", f"profile: {prof}"
assert prof.get("vid") == 0x1209 and prof.get("pid_value") == 0x5253, f"vid/pid: {prof}"
assert prof.get("vid_pid_provisional") is True, f"provisional: {prof}"
assert prof.get("descriptor_len") == 124, f"descriptor len: {prof}"
assert prof.get("input_report_size") == 15, f"report size: {prof}"
assert prof.get("input_report_size_legacy_assumption") == 26, f"legacy: {prof}"
assert prof.get("button_count") == 32 and prof.get("axis_count") == 5, f"counts: {prof}"
install = lines[4]
assert install.get("ok") is False and install.get("code") == "not-admin", f"install on Linux: {install}"
assert lines[5].get("code") == "unknown-command", f"unknown: {lines[5]}"
assert lines[6].get("code") == "profile-refused", f"brand refusal: {lines[6]}"
assert lines[7].get("code") == "no-controller", f"submit w/o create: {lines[7]}"
assert lines[8].get("bye") is True, f"exit: {lines[8]}"
print("verify-linux: OK — hello/version/describe-profile(15B)/install(not-admin)/unknown/brand-refusal/no-controller/exit")
PYEOF

rm -f verify-linux.stdout verify-linux.stderr
echo "verify-linux.sh: PASS"
