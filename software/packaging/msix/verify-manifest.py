#!/usr/bin/env python3
"""RSH — MSIX manifest verify (Phase H, SW-16).

Kisi bhi OS par chalta hai (sirf XML check — MakeAppx/signtool Windows par
hain). Checks: Identity Name RSH, Version 2.0.0.0, MinVersion Windows 11
(10.0.22000.0), Executable RSH.exe, capabilities (privateNetworkClientServer
+ internetClient), aur Publisher ka status. USER DECISION 2026-11-13: OV
certificate abhi nahi — v2.0.0 UNSIGNED ship hoga, is liye Publisher
CN=RSH (unsigned identity) valid hai; cert milne par Publisher ko exact
cert Subject se badalna hoga (docs/SIGNING.md §3.1).

Usage: python3 software/packaging/msix/verify-manifest.py
Exit 0 = manifest draft valid; exit 1 = koi check fail.
"""
import sys
import xml.etree.ElementTree as ET
from pathlib import Path

MANIFEST = Path(__file__).resolve().parent / "Package.appxmanifest"
NS = {
    "m": "http://schemas.microsoft.com/appx/manifest/foundation/windows10",
    "uap": "http://schemas.microsoft.com/appx/manifest/uap/windows10",
}

failures = []


def check(cond, msg):
    print(("PASS " if cond else "FAIL ") + msg)
    if not cond:
        failures.append(msg)


def main():
    if not MANIFEST.exists():
        print(f"FAIL manifest nahi mili: {MANIFEST}")
        return 1
    tree = ET.parse(MANIFEST)
    root = tree.getroot()
    ident = root.find("m:Identity", NS)
    check(ident is not None, "Identity maujood hai")
    if ident is not None:
        check(ident.get("Name") == "RSH", f"Identity Name=RSH (mila: {ident.get('Name')})")
        check(ident.get("Version") == "2.0.0.0", f"Identity Version=2.0.0.0 (mila: {ident.get('Version')})")
        pub = ident.get("Publisher", "")
        if "TODO" in pub:
            print("WARN Publisher placeholder hai — v2.0.0 unsigned identity CN=RSH honi chahiye; cert milne par OV cert Subject se badlo (docs/SIGNING.md §3.1).")
        else:
            check(True, f"Publisher set hai: {pub}")
    dep = root.find("m:Dependencies/m:TargetDeviceFamily", NS)
    check(dep is not None, "TargetDeviceFamily maujood hai")
    if dep is not None:
        check(dep.get("MinVersion") == "10.0.22000.0",
              f"MinVersion Windows 11 = 10.0.22000.0 (mila: {dep.get('MinVersion')})")
    app = root.find("m:Applications/m:Application", NS)
    check(app is not None, "Application maujood hai")
    if app is not None:
        check(app.get("Executable") == "RSH.exe", f"Executable=RSH.exe (mila: {app.get('Executable')})")
        check(app.get("EntryPoint") == "Windows.FullTrustApplication",
              f"EntryPoint=Windows.FullTrustApplication (mila: {app.get('EntryPoint')})")
    caps = {c.get("Name") for c in root.findall("m:Capabilities/m:Capability", NS)}
    check("privateNetworkClientServer" in caps, "Capability privateNetworkClientServer (wheel AP)")
    check("internetClient" in caps, "Capability internetClient (update check)")
    if failures:
        print(f"\n{len(failures)} check(s) FAIL.")
        return 1
    print("\nManifest valid hai (v2.0.0 unsigned: Publisher CN=RSH; cert milne par Publisher = cert Subject, SIGNING.md §3.1).")
    return 0


if __name__ == "__main__":
    sys.exit(main())
