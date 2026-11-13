#!/usr/bin/env python3
"""merge_bin.py host-mock test (FW-13, F15: merge fail = build fail).
SCons ke baghair: Import("env") stub karke merge_bin.py exec karta hai,
phir post-action callback ko fake env ke saath chalata hai.

Checks:
  1. _firmware_version() config.h se version parhta hai.
  2. Koi input missing ho (app/bootloader/partitions/boot_app0) ->
     RuntimeError (khamoshi se return NAHI — yehi silent-merge bug tha).
  3. Fake esptool chhota output likhe -> RuntimeError (size sanity, F15).
  4. Fake esptool sahi output likhe -> merge OK, output naam mein version.
Run: python3 -I tests-host/test_merge_bin_mock.py (run_tests.sh se bhi).
"""
import os
import subprocess
import sys
import tempfile

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
MERGE_BIN = os.path.join(REPO, "firmware", "rsh", "merge_bin.py")

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


class FakePlatform:
    def __init__(self, fw_dir, esptool_dir):
        self._fw = fw_dir
        self._esp = esptool_dir

    def get_package_dir(self, name):
        if name == "framework-arduinoespressif32":
            return self._fw
        if name == "tool-esptoolpy":
            return self._esp
        raise KeyError(name)


class FakeEnv:
    def __init__(self, build_dir, proj_dir, platform):
        self._build = build_dir
        self._proj = proj_dir
        self._platform = platform
        self.post_actions = []

    def subst(self, s):
        return {"$BUILD_DIR": self._build,
                "$PROJECT_DIR": self._proj,
                "$PYTHONEXE": sys.executable}.get(s, s)

    def PioPlatform(self):
        return self._platform

    def AddPostAction(self, target, func):
        self.post_actions.append((target, func))


def load_merge_bin(env):
    """merge_bin.py ko fake SCons env ke saath exec karo; post-action lo."""
    g = {"__name__": "merge_bin_mock", "__file__": MERGE_BIN}
    g["Import"] = lambda name: g.__setitem__("env", env)
    with open(MERGE_BIN, encoding="utf-8") as f:
        exec(compile(f.read(), MERGE_BIN, "exec"), g)  # noqa: S102 (test-only)
    assert len(env.post_actions) == 1, "merge_bin ne post-action register nahi ki"
    return g, env.post_actions[0][1]


def make_fake_esptool(path, out_size):
    """Fake esptool.py: `-o <out>` ke baad out_size bytes likhta hai."""
    with open(path, "w", encoding="utf-8") as f:
        f.write("import sys\n"
                "argv = sys.argv\n"
                "out = argv[argv.index('-o') + 1]\n"
                f"open(out, 'wb').write(b'\\xFF' * {out_size})\n")
    os.chmod(path, 0o755)


def main():
    print("== merge_bin host-mock (F15) ==")
    with tempfile.TemporaryDirectory() as tmp:
        proj = os.path.join(tmp, "proj")
        build = os.path.join(tmp, "build")
        fw = os.path.join(tmp, "fw")
        esp = os.path.join(tmp, "esp")
        for d in (proj, build, fw, esp):
            os.makedirs(d)
        os.makedirs(os.path.join(fw, "tools", "partitions"))
        with open(os.path.join(proj, "config.h"), "w", encoding="utf-8") as f:
            f.write('#define FIRMWARE_VERSION "4.0.0"\n')
        for name in ("firmware.bin", "bootloader.bin", "partitions.bin"):
            with open(os.path.join(build, name), "wb") as f:
                f.write(b"\xE9" * 1024)
        with open(os.path.join(fw, "tools", "partitions", "boot_app0.bin"), "wb") as f:
            f.write(b"\x00" * 1024)

        # 1. Version parsing (real config.h format).
        env = FakeEnv(build, proj, FakePlatform(fw, esp))
        make_fake_esptool(os.path.join(esp, "esptool.py"), 70000)
        g, merge_fn = load_merge_bin(env)
        check(g["_firmware_version"]() == "4.0.0", "version config.h se 4.0.0")

        # 2. Missing input -> RuntimeError (har input ke liye).
        for missing in ("firmware.bin", "bootloader.bin", "partitions.bin"):
            os.rename(os.path.join(build, missing), os.path.join(tmp, missing))
            try:
                merge_fn(None, None, env)
                check(False, f"missing {missing} -> RuntimeError")
            except RuntimeError as e:
                check("missing input" in str(e), f"missing {missing} -> RuntimeError")
            os.rename(os.path.join(tmp, missing), os.path.join(build, missing))
        os.rename(os.path.join(fw, "tools", "partitions", "boot_app0.bin"),
                  os.path.join(tmp, "boot_app0.bin"))
        try:
            merge_fn(None, None, env)
            check(False, "missing boot_app0.bin -> RuntimeError")
        except RuntimeError:
            check(True, "missing boot_app0.bin -> RuntimeError")
        os.rename(os.path.join(tmp, "boot_app0.bin"),
                  os.path.join(fw, "tools", "partitions", "boot_app0.bin"))

        # 3. Chhota output -> RuntimeError (size sanity).
        make_fake_esptool(os.path.join(esp, "esptool.py"), 100)
        try:
            merge_fn(None, None, env)
            check(False, "tiny output (100 B) -> RuntimeError")
        except RuntimeError as e:
            check("na-mukammal" in str(e), "tiny output (100 B) -> RuntimeError")

        # 4. Happy path: output banta hai, naam mein version hai.
        make_fake_esptool(os.path.join(esp, "esptool.py"), 70000)
        merge_fn(None, None, env)
        out = os.path.join(build, "RSH-Firmware-DevKitV1-v4.0.0.bin")
        check(os.path.isfile(out) and os.path.getsize(out) == 70000,
              "merge OK, RSH-Firmware-DevKitV1-v4.0.0.bin bana")

    print(f"MERGE_BIN MOCK {'PASSED' if not FAILS else 'FAILED'} — "
          f"{CHECKS} checks, {len(FAILS)} failures")
    return 1 if FAILS else 0


if __name__ == "__main__":
    raise SystemExit(main())
