"""PlatformIO post-build script: create the merged factory image (flash at 0x0).

Runs after `pio run`. Output:
  .pio/build/<env>/ApexLink-Firmware-DevKitV1-v<FIRMWARE_VERSION>.bin
"""
import os
import re
import sys

Import("env")  # noqa: F821  (provided by PlatformIO/SCons)


def _firmware_version():
    cfg = os.path.join(env.subst("$PROJECT_DIR"), "config.h")  # noqa: F821
    try:
        with open(cfg, encoding="utf-8") as f:
            m = re.search(r'#define\s+FIRMWARE_VERSION\s+"([^"]+)"', f.read())
            if m:
                return m.group(1)
    except OSError:
        pass
    return "unknown"


def merge_bin(source, target, env):  # noqa: F811
    build_dir = env.subst("$BUILD_DIR")
    app = os.path.join(build_dir, "firmware.bin")
    boot = os.path.join(build_dir, "bootloader.bin")
    parts = os.path.join(build_dir, "partitions.bin")

    # boot_app0.bin lives in the Arduino-ESP32 framework package.
    fw_dir = env.PioPlatform().get_package_dir("framework-arduinoespressif32")
    boot_app0 = os.path.join(fw_dir, "tools", "partitions", "boot_app0.bin")

    for p in (app, boot, parts, boot_app0):
        if not os.path.isfile(p):
            sys.stderr.write("merge_bin: missing %s — skipping merge\n" % p)
            return

    out = os.path.join(
        build_dir, "ApexLink-Firmware-DevKitV1-v%s.bin" % _firmware_version())

    esptool_dir = env.PioPlatform().get_package_dir("tool-esptoolpy")
    cmd = [
        env.subst("$PYTHONEXE"),
        os.path.join(esptool_dir, "esptool.py"),
        "--chip", "esp32", "merge_bin",
        "-o", out,
        "--flash_mode", "dio",
        "--flash_freq", "40m",
        "--flash_size", "4MB",
        "0x1000", boot,
        "0x8000", parts,
        "0xe000", boot_app0,
        "0x10000", app,
    ]
    env.Execute(cmd)
    print("merge_bin: wrote %s" % out)


env.AddPostAction("$BUILD_DIR/${PROGNAME}.bin", merge_bin)  # noqa: F821
