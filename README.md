# RSH — Real Sim Hub

[![CI](https://github.com/dreamsofts34-creator/RSH/actions/workflows/ci.yml/badge.svg)](https://github.com/dreamsofts34-creator/RSH/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/dreamsofts34-creator/RSH)](https://github.com/dreamsofts34-creator/RSH/releases/latest)
[![.NET](https://img.shields.io/badge/.NET-10.0-512BD4)](https://dotnet.microsoft.com/en-us/download/dotnet/10.0)
[![Firmware](https://img.shields.io/badge/firmware-v4.0.1-blue)](docs/RELEASE-NOTES-v2.0.4.md)
[![Protocol](https://img.shields.io/badge/protocol-v2.1-blue)](docs/PROTOCOL.md)

**RSH is a DIY sim-racing steering wheel system: an ESP32 board reads
your wheel hardware and streams it to a Windows app, which feeds a
vJoy virtual joystick that any racing game can bind like a real wheel.**

- **App v2.0.4** · **Firmware v4.0.1** · **Protocol v2.1** — see
  [docs/RELEASE-NOTES-v2.0.4.md](docs/RELEASE-NOTES-v2.0.4.md)
- Modular: attach only the modules you want, assign pins from the app,
  tune each module separately
- Principle: **firmware dumb, PC smart** — all calibration and
  processing live on the PC

---

## Features

- **Modular wheel** — Steering, Accelerator, Brake, Clutch, Handbrake,
  Shifter, Horn, Buttons, Encoders, LED — each with its own on/off
  toggle and tuning (deadzone, curve, smoothing, invert)
- **200 Hz end to end** — the board samples at 200 Hz, the PC engine
  runs at 200 Hz, with a 250 ms watchdog that sends NEUTRAL if data
  goes stale
- **Two transports** — Wi-Fi UDP (board SoftAP `RSH`) or wired
  USB-serial at 921600 baud; switch in Settings → Connection
- **Runtime pin assignment** — pins are configured from the app and
  pushed to the board; a config transaction (stage → commit → confirm
  → rollback) means a bad pin map can never brick the wheel
- **Per-game and per-car profiles** — profiles switch automatically
  by foreground game and by car (Assetto Corsa / ACC / iRacing
  telemetry), with 3-generation backup
- **Game launcher** — launch Assetto Corsa, ACC, iRacing and
  Dirt Rally 2.0 from the Dashboard
- **vJoy output (default)** with an experimental HIDMaestro backend
- **Signed firmware** — Ed25519 signature verified before apply,
  with anti-downgrade; control channel authenticated with HMAC-SHA256
  and a per-device key
- **Modern WPF UI** — 8 screens, Light + Dark + High Contrast themes,
  game launcher on the Dashboard, GPIO pin map in Settings.
  On Windows 10 the app opens a real plain window
  (`ClassicMainWindow`); on Windows 11 it uses the FluentWindow UI
- **Safe mode** — BOOT-hold at power-on recovers a bad config

## Quickstart (users)

1. **Download the latest release:**
   [RSH v2.0.4](https://github.com/dreamsofts34-creator/RSH/releases/tag/v2.0.4)
   — get `RSH-windows-x64.zip` (~16.7 MB, 43 files) and extract it to
   a new folder.
2. **Install the .NET 10 Desktop Runtime (x64) — required:**
   <https://dotnet.microsoft.com/en-us/download/dotnet/10.0>
   (Run apps → **.NET Desktop Runtime 10.0.x**), or
   `winget install Microsoft.DotNet.DesktopRuntime.10`.
   Check with `dotnet --list-runtimes` — it must list
   `Microsoft.WindowsDesktop.App 10.x`.
3. **Install the vJoy driver (once, as administrator):**
   BrunnerInnovation **v2.2.2.0**, device enabled in vJoyConf with
   axes X/Y/Z.
4. **Flash the firmware (v4.0.1) if your board runs v4.0.0:**
   the merged image `RSH-Firmware-DevKitV1-v4.0.1.bin` is in the
   release — v4.0.1 is **not** byte-identical to v4.0.0, so reflash.
   Steps: [docs/FLASH-GUIDE.md](docs/FLASH-GUIDE.md).
5. **Run `RSH.exe`** (no flags needed), join the `RSH` Wi-Fi network,
   and run the setup wizard once (Connect → Pins → Calibrate).
   Then bind the vJoy device in your game.

> **Unsigned app:** RSH is not code-signed yet, so Windows
> SmartScreen will warn on first run — click "More info" → "Run
> anyway". Firmware signing (Ed25519) is unaffected.
> Full user guide: [docs/USER-GUIDE.md](docs/USER-GUIDE.md).

## Architecture

```
[Steering pot] [Hall pedals] [Buttons] [Encoders] [LED strip]
        \            |            |          |          /
              ESP32 DevKit V1 — firmware v4.0.1
              200 Hz loop · Wi-Fi UDP 200 pps / USB-serial
                        |
              Windows PC — app v2.0.4
              WheelEngine (200 Hz) → ProcessingPipeline
              (map → deadzone → curve → One Euro → pedal ramp)
                        |
              vJoy virtual joystick → racing game
```

Read the full architecture docs in [`docs/`](docs/README.md):

- [ARCHITECTURE.md](docs/ARCHITECTURE.md) — system overview
- [FIRMWARE.md](docs/FIRMWARE.md) — firmware internals
- [SOFTWARE.md](docs/SOFTWARE.md) — PC app internals
- [PROTOCOL.md](docs/PROTOCOL.md) — protocol v2.1 summary
  (authority: [PROTOCOL-V2.1.md](docs/PROTOCOL-V2.1.md))
- [SECURITY.md](docs/SECURITY.md) — security model
- [TESTING.md](docs/TESTING.md) — Virtual Test Rig, suites, gates

## Repository structure

```
README.md                  <- this file
docs/                      <- documentation (index: docs/README.md)
firmware/rsh/              <- ESP32 firmware (PlatformIO, env esp32dev)
software/
  RSH.slnx                 <- solution (.NET 10, WPF)
  src/RSH.Core/            <- protocol, engine, pipeline, profiles (no UI)
  src/RSH.App/             <- WPF app (8 screens, wizard, tray)
  src/RSH.Output.VJoy/     <- vJoy backend (default)
  src/RSH.Output.HIDMaestro/ <- HIDMaestro backend (experimental)
  src/RSH.RenderTest/      <- minimal plain-Window render test
  tests/                   <- RSH.Tests, RSH.VJoy.Tests,
                              RSH.HIDMaestro.Tests, RSH.App.Tests
  tools/                   <- RSH.Simulator, RSH.Benchmarks
  packaging/               <- MSIX, winget, firewall, SBOM
tests-host/                <- firmware host tests (C++17, no Arduino)
tools-perf/                <- Measure-RSH.ps1 (perf baseline)
.github/workflows/         <- ci.yml (per push/PR),
                              release.yml (tag v* → GitHub Release)
```

## Development

Prerequisites: **.NET 10 SDK (10.0.100)** and **PlatformIO**
(ESP32 platform) for firmware.

```powershell
# Build the app
dotnet build software/RSH.slnx -c Release

# Run the tests (RSH.App.Tests is Windows-only; the rest run on Linux too)
dotnet test software/RSH.slnx -c Release

# Build the firmware
cd firmware/rsh
pio run -e esp32dev

# Firmware host tests (no Arduino needed)
tests-host/run_tests.sh
```

Tests run in the **Virtual Test Rig** on local builds
(`~/workspace/projects/rsh/test-env/` in the project workspace) —
25 scenarios, fuzz, fault injection, soaks. Real-board tests happen
once, at the final production release (A7 runbook:
[docs/A7-RUNBOOK.md](docs/A7-RUNBOOK.md)). See
[docs/TESTING.md](docs/TESTING.md).

**Latest gate (v2.0.4, 2026-11-14):** build 0 errors ·
917 .NET tests pass, 0 fail (RSH.Tests 783 + VJoy 56 + HIDMaestro 78) ·
host tests 4430 checks, 0 failures · rig 25/25 scenarios (soak 200/s) ·
benchmarks 15/15 · `pio run` SUCCESS.

## Contributing

This is a personal DIY project by Dreamsofts. Issues and pull
requests are welcome:

- Keep the phase discipline: **build → test (rig) → find bugs →
  fix → gate → next step**. No step is skipped.
- Do not send release zips in chat/issues — builds go to
  [GitHub Releases](https://github.com/dreamsofts34-creator/RSH/releases).
- Never commit signing keys, passwords or `*.pem` files
  (`.gitignore` blocks them) — the release signing key lives
  outside this repo.
- UI changes must respect the locked design in
  [docs/UI-LOCKED.md](docs/UI-LOCKED.md).

## Security

See [docs/SECURITY.md](docs/SECURITY.md) and
[docs/THREAT-MODEL.md](docs/THREAT-MODEL.md). To report a
vulnerability, open a GitHub issue with "SECURITY:" in the title and
do not publish an exploit before it is fixed.

## License

**TBD — no license file yet.** The project is public on GitHub but
no licence has been chosen; until one is added, the code is
source-available for viewing only. (Suggested: MIT — to be confirmed
with the owner.)
