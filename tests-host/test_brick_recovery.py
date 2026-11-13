#!/usr/bin/env python3
"""Brick recovery host-mock test (FW-14, R-M12).
Bina real board ke: recovery ke liye zaroori merged factory bin ki
structure validate karta hai + esptool recovery command banata hai.

Checks:
  Mock (hamesha chalta hai):
    1. Fake merged image (offsets par dummy parts) structure pass karti hai.
    2. Khaali/ghalat image reject hoti hai (ghalat magic / chhota size).
    3. Recovery command theek banta hai (0x0 offset, chip esp32, baud).
  Real (merged bin mile to; --require-real se lazmi):
    4. Asal merged bin: size, bootloader magic 0xE9 @ 0x1000, partition
       magic 0xAA50 @ 0x8000, app magic 0xE9 @ 0x10000 + SHA256.
Run:
  python3 tests-host/test_brick_recovery.py [--merged-bin <path>] [--require-real]
  (Default merged-bin location: firmware/rsh/.pio/build/esp32dev/RSH-Firmware-DevKitV1-v*.bin)
"""
import argparse
import glob
import hashlib
import os
import sys

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
DEFAULT_GLOB = os.path.join(REPO, "firmware", "rsh", ".pio", "build",
                            "esp32dev", "RSH-Firmware-DevKitV1-v*.bin")

CHECKS = 0
FAILS = []


def check(cond, label):
    global CHECKS
    CHECKS += 1
    if cond:
        print(f"  ok: {label}")
    else:
        FAILS.append(label)
        print(f"  FAIL: {label}")


def validate_merged(data):
    """Merged factory image ki structure: (ok, reason)."""
    if len(data) < 0x10000 + 1024:
        return False, f"too small ({len(data)} B)"
    if data[0x1000] != 0xE9:
        return False, "bootloader magic != 0xE9 @ 0x1000"
    if data[0x8000:0x8002] != bytes([0xAA, 0x50]):
        return False, "partition magic != 0xAA50 @ 0x8000"
    if data[0x10000] != 0xE9:
        return False, "app magic != 0xE9 @ 0x10000"
    return True, "ok"


def recovery_command(port, merged_bin, baud=921600):
    """esptool recovery command (BRICK-RECOVERY.md) — argv list."""
    return ["esptool.py", "--chip", "esp32", "--port", port,
            "--baud", str(baud), "write_flash", "0x0", merged_bin]


def make_fake_merged():
    """Fake merged image: offsets par sahi magics, baaqi 0xFF."""
    size = 0x10000 + 4096
    img = bytearray([0xFF] * size)
    img[0x1000] = 0xE9
    img[0x8000:0x8002] = bytes([0xAA, 0x50])
    img[0x10000] = 0xE9
    return bytes(img)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--merged-bin", default=None)
    ap.add_argument("--require-real", action="store_true")
    args = ap.parse_args()

    print("== brick recovery host-mock (FW-14, R-M12) ==")
    # --- Mock: fake merged image ---
    ok, reason = validate_merged(make_fake_merged())
    check(ok, f"fake merged image validates ({reason})")
    bad = bytearray(make_fake_merged())
    bad[0x1000] = 0x00
    ok, _ = validate_merged(bytes(bad))
    check(not ok, "bad bootloader magic rejected")
    bad = bytearray(make_fake_merged())
    bad[0x8000] = 0x00
    ok, _ = validate_merged(bytes(bad))
    check(not ok, "bad partition magic rejected")
    bad = bytearray(make_fake_merged())
    bad[0x10000] = 0x00
    ok, _ = validate_merged(bytes(bad))
    check(not ok, "bad app magic rejected")
    check(not validate_merged(b"")[0], "empty image rejected")
    check(not validate_merged(bytes(100))[0], "tiny image rejected")
    cmd = recovery_command("/dev/ttyUSB0", "RSH-Firmware-DevKitV1-v4.0.0.bin")
    check(cmd == ["esptool.py", "--chip", "esp32", "--port", "/dev/ttyUSB0",
                  "--baud", "921600", "write_flash", "0x0",
                  "RSH-Firmware-DevKitV1-v4.0.0.bin"],
          "recovery command: esptool write_flash 0x0 (baud 921600)")
    check(recovery_command("COM3", "x.bin", baud=115200)[6] == "115200",
          "recovery command: fallback baud 115200")

    # --- Real: asal merged bin (pio run / release artifact) ---
    path = args.merged_bin
    if not path:
        found = sorted(glob.glob(DEFAULT_GLOB))
        path = found[-1] if found else None
    if path and os.path.isfile(path):
        with open(path, "rb") as f:
            data = f.read()
        ok, reason = validate_merged(data)
        check(ok, f"real merged bin validates: {os.path.basename(path)} ({reason})")
        print(f"  real bin: {len(data)} B, "
              f"sha256={hashlib.sha256(data).hexdigest()[:16]}...")
    else:
        msg = "real merged bin nahi mila (pio run ke baad aata hai)"
        if args.require_real:
            check(False, msg)
        else:
            print(f"  SKIP (real): {msg}")

    print(f"BRICK RECOVERY MOCK {'PASSED' if not FAILS else 'FAILED'} — "
          f"{CHECKS} checks, {len(FAILS)} failures")
    return 1 if FAILS else 0


if __name__ == "__main__":
    raise SystemExit(main())
