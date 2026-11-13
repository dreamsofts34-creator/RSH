# RSH — Third-Party Licenses (v2.0.0)

**Version 2.0.0 — app v2.0.0 · firmware v4.0.0 · protocol v2.1**
Authority: Plan v2.3 §5 Phase H (R-M29: NuGet + vJoy + ESP32 libs, CycloneDX/SPDX).
Machine-readable SBOM: `docs/SBOM.cyclonedx.json` (CycloneDX 1.5) — ye doc usi
se bana hai (`software/packaging/sbom/generate-sbom.py` dobara chalao to dono
ek saath update hote hain). SPDX format mein SBOM abhi nahi hai — CycloneDX
hi release SBOM hai; SPDX chahiye ho to CycloneDX se convert karo.

**License note:** neeche licenses NuGet.org / project pages ke mutabiq hain —
release se pehle har license dobara verify karo (package ka apna LICENSE file
ya NuGet listing). Koi license yahan "assume" karke nahi likhi gayi; jo package
ki license verify na ho sake us ke saamne "— check karo" likha hoga.

## Components

| Component | Version | License |
|---|---|---|
| CommunityToolkit.Mvvm | 8.4.2 | MIT |
| HarfBuzzSharp | 8.3.1.1 | MIT |
| HarfBuzzSharp.NativeAssets.Linux | 8.3.1.1 | MIT |
| HarfBuzzSharp.NativeAssets.Win32 | 8.3.1.1 | MIT |
| HarfBuzzSharp.NativeAssets.macOS | 8.3.1.1 | MIT |
| OpenTK | 4.9.4 | MIT |
| OpenTK.Audio.OpenAL | 4.9.4 | MIT |
| OpenTK.Compute | 4.9.4 | MIT |
| OpenTK.Core | 4.9.4 | MIT |
| OpenTK.GLWpfControl | 4.3.3 | MIT |
| OpenTK.Graphics | 4.9.4 | MIT |
| OpenTK.Input | 4.9.4 | MIT |
| OpenTK.Mathematics | 4.9.4 | MIT |
| OpenTK.Windowing.Common | 4.9.4 | MIT |
| OpenTK.Windowing.Desktop | 4.9.4 | MIT |
| OpenTK.Windowing.GraphicsLibraryFramework | 4.9.4 | MIT |
| OpenTK.redist.glfw | 3.4.0.44 | MIT |
| ScottPlot | 5.1.59 | MIT |
| ScottPlot.WPF | 5.1.59 | MIT |
| Serilog | 4.4.0 | Apache-2.0 |
| Serilog.Sinks.File | 7.0.0 | Apache-2.0 |
| SkiaSharp | 3.119.0 | MIT |
| SkiaSharp.HarfBuzz | 3.119.0 | MIT |
| SkiaSharp.NativeAssets.Linux.NoDependencies | 3.119.0 | MIT |
| SkiaSharp.NativeAssets.Win32 | 3.119.0 | MIT |
| SkiaSharp.NativeAssets.macOS | 3.119.0 | MIT |
| SkiaSharp.Views.Desktop.Common | 3.119.0 | MIT |
| SkiaSharp.Views.WPF | 3.119.0 | MIT |
| System.IO.FileSystem.AccessControl | 5.0.0 | MIT |
| System.IO.Ports | 10.0.0 | MIT |
| WPF-UI | 4.3.0 | MIT |
| WPF-UI.Abstractions | 4.3.0 | MIT |
| WPF-UI.Tray | 4.3.0 | MIT |
| vJoy | 2.2.2.0 | MIT |
| Arduino-ESP32 framework | espressif32@6.9.0 | LGPL-2.1-only |
| .NET runtime | 10.0.100 | MIT |

## Notes

- **vJoy (BrunnerInnovation 2.2.2.0, MIT):** external driver hai — RSH app
  bundle nahi karti (D18). User khud install karta hai; RSH sirf
  `vJoyInterface.dll` ko resolve karta hai (registry → Program Files).
- **Arduino-ESP32 framework (espressif32@6.9.0, LGPL-2.1-only):** firmware
  framework — `firmware/rsh/platformio.ini` mein pinned hai.
- **.NET 10 runtime (MIT):** self-contained publish ka runtime
  (`software/global.json` pin 10.0.100).
- **Apne components (RSH.Core, RSH.Output.VJoy, RSH.App):** is list mein nahi
  hain — wo third-party nahi, RSH khud hai.
- **License texts:** har component ka poora license text us ke apne repo /
  NuGet package mein hota hai; ye doc sirf license IDs list karta hai.
