#!/usr/bin/env python3
"""RSH — SBOM generator (CycloneDX 1.5, Phase H / SW-16, R-M29).

Sources (ground truth, koi haath se likhi version nahi):
  - software/src/RSH.App/packages.lock.json  (NuGet, Direct + Transitive)
  - software/src/RSH.Core/packages.lock.json (NuGet, Direct + Transitive)
  - firmware/rsh/platformio.ini             (espressif32 platform pin)
  - Manual entries: vJoy 2.2.2.0 (plan-verified MIT, NuGet mein nahi),
    .NET 10 runtime (self-contained publish ka runtime).
Licenses: NuGet/project pages ke mutabiq (neeche LICENSES map) — release
se pehle dobara verify karo (docs/THIRD-PARTY-LICENSES.md dekho).
`runtime.*` native packs SBOM mein nahi aate (wo .NET runtime ke
platform-specific hisse hain, alag components nahi).

Usage (repo root se): python3 software/packaging/sbom/generate-sbom.py
Output: docs/SBOM.cyclonedx.json
"""
import json
import sys
import uuid
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent.parent.parent
APP_VERSION = "2.0.0"
FW_VERSION = "4.0.0"

# License per package (NuGet.org / project page — release se pehle verify).
LICENSES = {
    "WPF-UI": "MIT", "WPF-UI.Tray": "MIT", "WPF-UI.Abstractions": "MIT",
    "CommunityToolkit.Mvvm": "MIT",
    "ScottPlot.WPF": "MIT", "ScottPlot": "MIT",
    "Serilog": "Apache-2.0", "Serilog.Sinks.File": "Apache-2.0",
    "System.IO.Ports": "MIT",
    "HarfBuzzSharp": "MIT",
    "HarfBuzzSharp.NativeAssets.Linux": "MIT",
    "HarfBuzzSharp.NativeAssets.macOS": "MIT",
    "HarfBuzzSharp.NativeAssets.Win32": "MIT",
    "OpenTK": "MIT", "OpenTK.Audio.OpenAL": "MIT", "OpenTK.Compute": "MIT",
    "OpenTK.Core": "MIT", "OpenTK.GLWpfControl": "MIT", "OpenTK.Graphics": "MIT",
    "OpenTK.Input": "MIT", "OpenTK.Mathematics": "MIT",
    "OpenTK.redist.glfw": "MIT", "OpenTK.Windowing.Common": "MIT",
    "SkiaSharp": "MIT", "SkiaSharp.Views.WPF": "MIT",
    "SkiaSharp.HarfBuzz": "MIT",
    "SkiaSharp.NativeAssets.Linux.NoDependencies": "MIT",
    "SkiaSharp.NativeAssets.Win32": "MIT",
    "SkiaSharp.NativeAssets.macOS": "MIT",
    "SkiaSharp.Views.Desktop.Common": "MIT",
    "OpenTK.Windowing.Desktop": "MIT",
    "OpenTK.Windowing.GraphicsLibraryFramework": "MIT",
    "System.IO.FileSystem.AccessControl": "MIT",
}


def load_lock(path):
    data = json.loads(Path(path).read_text())
    out = {}
    for tfm, deps in data.get("dependencies", {}).items():
        for name, info in deps.items():
            if name.startswith("runtime."):
                continue  # .NET runtime native packs — alag component nahi
            if name.startswith("rsh."):
                continue  # Apne project references (RSH.Core / RSH.Output.VJoy) — third-party nahi
            out[name] = info.get("resolved", "")
    return out


def nuget_component(name, version):
    comp = {
        "type": "library",
        "name": name,
        "version": version,
        "purl": f"pkg:nuget/{name}@{version}",
    }
    lic = LICENSES.get(name)
    if lic:
        comp["licenses"] = [{"license": {"id": lic}}]
    return comp


def main():
    pkgs = {}
    for lock in (REPO / "software/src/RSH.App/packages.lock.json",
                 REPO / "software/src/RSH.Core/packages.lock.json"):
        if not lock.exists():
            print(f"FAIL lock file nahi mili: {lock}")
            return 1
        pkgs.update(load_lock(lock))
    components = [nuget_component(n, v) for n, v in sorted(pkgs.items())]
    # Manual entries (NuGet mein nahi — plan/docs se).
    components += [
        {"type": "library", "name": "vJoy", "version": "2.2.2.0",
         "description": "BrunnerInnovation vJoy virtual joystick driver (external driver, app bundle nahi karti — D18)",
         "licenses": [{"license": {"id": "MIT"}}],
         "externalReferences": [{"type": "website",
                                  "url": "https://github.com/BrunnerInnovation/vJoy/releases"}]},
        {"type": "library", "name": "Arduino-ESP32 framework", "version": "espressif32@6.9.0",
         "description": "ESP32 firmware framework (platformio.ini pin)",
         "licenses": [{"license": {"id": "LGPL-2.1-only"}}]},
        {"type": "library", "name": ".NET runtime", "version": "10.0.100",
         "description": "Self-contained publish ka runtime (software/global.json pin)",
         "licenses": [{"license": {"id": "MIT"}}]},
    ]
    bom = {
        "bomFormat": "CycloneDX",
        "specVersion": "1.5",
        "serialNumber": f"urn:uuid:{uuid.uuid5(uuid.NAMESPACE_URL, 'https://github.com/dreamsofts34-creator/RSH/releases/tag/v2.0.0')}",
        "version": 1,
        "metadata": {
            "timestamp": "2026-11-13T00:00:00Z",
            "component": {
                "type": "application",
                "name": "RSH",
                "version": APP_VERSION,
                "description": "Real Sim Hub — app v2.0.0 / firmware v4.0.0 / protocol v2.1",
            },
        },
        "components": components,
    }
    out = REPO / "docs/SBOM.cyclonedx.json"
    out.write_text(json.dumps(bom, indent=2) + "\n")
    print(f"SBOM likhi: {out} ({len(components)} components, "
          f"{len(pkgs)} NuGet + 3 manual)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
