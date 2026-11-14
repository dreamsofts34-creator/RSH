# RSH — PC App Architecture

> **Versions:** App **v2.0.4** · Firmware **v4.0.1** · Protocol **v2.1**
> (2026-11-14). System overview: [`ARCHITECTURE.md`](ARCHITECTURE.md).
> UI rules (locked): [`UI-LOCKED.md`](UI-LOCKED.md).

## Stack

- **Language / runtime:** C# · .NET 10 LTS (SDK 10.0.100, `software/global.json`).
- **UI:** WPF + **WPF-UI 4.3.0** (`WPF-UI`, `WPF-UI.Tray`),
  CommunityToolkit.Mvvm 8.4.2, ScottPlot.WPF 5.1.59 (charts).
- **Logging:** Serilog 4.4.0 (file sink 7.0.0).
- **Crypto:** NSec.Cryptography 24.4.0 (Ed25519 firmware verification).
- **Solution:** `software/RSH.slnx`. Target: `net10.0-windows`, x64.
  Floor: **Windows 10 22H2 (build 19045)** and Windows 11.
  (The original plan said Windows 11 only; the user's PC is
  Windows 10, so Win10 is a supported floor now.)

## Projects

| Project | Job |
|---|---|
| `src/RSH.Core` | Everything headless: protocol, transports, engine, pipeline, profiles, security, telemetry. No UI, fully unit-tested. |
| `src/RSH.App` | WPF app: 8 screens, 7-step setup wizard, tray icon, autostart, single instance. |
| `src/RSH.Output.VJoy` | vJoy backend (**default**). P/Invoke into the vJoy driver (BrunnerInnovation 2.2.2.0). |
| `src/RSH.Output.HIDMaestro` | HIDMaestro backend (**experimental**). Real v2.1.0 DLL vendored in `lib/`. No UI toggle yet — `settings.json` only; vJoy stays the default. |
| `src/RSH.RenderTest` | Minimal plain-Window app (no NuGet deps) to prove WPF renders on a PC. |
| `tests/RSH.Tests` | Core test suite (783 pass at the v2.0.4 gate). |
| `tests/RSH.VJoy.Tests` | vJoy interop tests (56 pass). |
| `tests/RSH.HIDMaestro.Tests` | HIDMaestro tests (78 pass). |
| `tests/RSH.App.Tests` | App tests — **Windows-only** (runs in CI, not on Linux). |
| `tools/RSH.Simulator` | Fake board for testing without hardware. |
| `tools/RSH.Benchmarks` | Pipeline / parse / E2E benchmarks with thresholds. |

## The engine: `WheelEngine`

`RSH.Core/Pipeline/WheelEngine.cs` — the heart of the app. A
dedicated thread runs a **200 Hz loop (5 ms tick)**:

```
latest packet → ProcessingPipeline → IJoystickOutput (vJoy / HIDMaestro)
```

Rules (all enforced in code, all tested):

1. **Watchdog:** data older than **250 ms** → output goes NEUTRAL.
2. **The loop never dies:** output errors, source errors and
   processing exceptions are counted (`LoopErrorCount`), output is
   neutralised, and the loop retries (reconnect every 2 s). The loop
   keeps running even with output disabled or no device found.
3. **Bounded stop:** `Stop()` waits at most ~5 s for the thread.
4. **The UI never blocks the engine:** the UI reads a snapshot at
   30 Hz; the engine never calls into the UI. No I/O on the UI thread
   (performance rules U-P1–U-P10).

## The pipeline: `ProcessingPipeline`

`RSH.Core/Processing/ProcessingPipeline.cs` — per tick, per module:

```
map (raw → mV / normalised) → deadzone → curve → One Euro smoothing
→ pedal ramp fusion → output state
```

- **Per-module tuning:** each of the 10 modules (Steering,
  Accelerator, Brake, Clutch, Handbrake, Shifter, Horn, Buttons,
  Encoders, LED) has its own on/off toggle and tuning
  (`PipelinePlan` / `ModuleTuning`). A disabled module emits nothing.
- **Calibration:** mV-domain, per-axis min/centre/max with a span
  check (≥ 660 mV, i.e. ≥ 20% of the 0–3300 mV range). Old unit-less
  calibrations are kept verbatim and flagged `NeedsRecalibration` —
  never silently reinterpreted as mV.
- **Encoders:** cumulative i16 deltas; encoder→button mapping
  (buttons 65–72) and pulses/debounce are per-module settings.
- **Handbrake:** mapped to the vJoy **WSlider** axis (HID usage 0x36).
- **Zero allocation on the hot path:** 0 B/tick (benchmark-gated).

## Transports (`RSH.Core/Transport/`)

| Class | Job |
|---|---|
| `UdpWheelSource` | UDP stream (board → PC, port 4211) + `SequenceTracker` (reboot-safe dedup) |
| `SerialWheelSource` / `SerialLink` / `SerialFrame` | Wired USB-serial, `RSHS` frames, 921600 baud |
| `SerialDeviceDetector` | COM-port auto-detect (skips dead ports) |
| `HelloClient` | HELO to port 4210 (~1 Hz), parses the 92 B HELLO reply |
| `ControlClient` / `ControlTransport` | Authenticated control channel (HMAC), retry with same reqId + new counter |
| `GetCapsClient` | `GET_CAPS` — what the board can do |
| `BoardSession` | Session state: key, nonce, pairing window, unpaired flag |
| `NtpPing` | NTP-style ping for honest end-to-end latency |
| `TransportKind` | `Udp` / `Serial` switch (Settings → Connection) |

The UDP sender filter is refreshed on `StartRuntime` (a Windows
loopback quirk was fixed in v2.0.4). HELLO/stream spoofing is guarded
by an expected-remote-address filter.

## Profiles (`RSH.Core/Profiles/`)

- **Profile v2** (`WheelProfile`): board MAC bound, per-module
  tuning, calibration, pin map, game preferences.
- **3-generation backup:** every save rotates
  `current → .bak1 → .bak2`; a corrupt file never overwrites a good
  backup (verified by test).
- **Per-car profiles:** telemetry readers (Assetto Corsa, ACC,
  iRacing shared memory) resolve the current car and auto-switch the
  profile (`TelemetryWatcher`, `CarProfileResolver`).
- **Per-game profiles:** foreground-process detection switches the
  profile when a game gains focus.
- **Game launcher:** the Dashboard launches Assetto Corsa, ACC,
  iRacing and Dirt Rally 2.0 (Steam). Real-launch verification is
  still open — the compatibility matrix is a draft
  ([`GAME-COMPATIBILITY.md`](GAME-COMPATIBILITY.md)).

## Output backends (`RSH.Core/Output/`)

- `IJoystickOutput` is the abstraction; `OutputBackend` selects it.
- **vJoy (default):** buttons 1–32 (higher buttons are rejected,
  never clamped onto 1–32), axes, POV, handbrake on WSlider.
  Requires the BrunnerInnovation v2.2.2.0 driver, device enabled in
  vJoyConf with axes X/Y/Z.
- **HIDMaestro (experimental):** user-mode alternative, evaluated in
  Phase G ([`HIDMAESTRO-EVALUATION.md`](HIDMAESTRO-EVALUATION.md)).
  78/78 tests pass; needs real-Windows verification before it can
  become the default.

## The UI (`RSH.App`)

**8 screens** in the sidebar (Dashboard · Tuning · Setup · Buttons ·
Output · Profiles · Monitor · Settings). Tuning shows one module at
a time as a card grid; Settings holds the GPIO pin map, module
toggles, theme (Light/Dark + High Contrast), transport switch and
data management. Text is professional English, localised via resx
(en / ur / hi, 337 keys); the theme accent is violet `#6C4CF1`,
light background `#F5F3FF`.

**Windows 10 path — `ClassicMainWindow`:** on Windows 10 (build
< 22000, detected with RtlGetVersion-based
`OperatingSystem.IsWindowsVersionAtLeast`, cross-checked against
`Environment.OSVersion`) the app opens a **real plain
`System.Windows.Window`** — OS title bar, no FluentWindow, no
`ui:TitleBar`, literal text, system brushes. On Windows 11 nothing
changes (normal FluentWindow). In classic mode the FluentWindow
setup wizard is skipped (the Setup screen covers the same steps)
and the change-password dialog uses its plain variant too.
The window title always shows the version
(`RSH — Real Sim Hub v2.0.4`), so a photo alone proves which binary
is running. Startup milestones are written to
`%TEMP%\RSH\startup-status.txt` and logs fall back to
`%TEMP%\RSH\logs` if `%AppData%\RSH\logs` is not writable.

**Other app behaviour:** single instance (version-aware mutex +
named pipe, older versions are terminated fail-closed), autostart
with stale-entry repair, tray icon (hide only works when the tray
icon actually exists), 7-step setup wizard (Connect → Pins →
Calibrate → …), firmware upload gate (validates the image and its
Ed25519 signature before upload; in classic mode use the browser
Web OTA instead — see `docs/RELEASE-NOTES-v2.0.4.md`).

> **Note:** `Views/Pages/ConnectionPage` and `CalibrationPage` are
> orphan pages (superseded by the Setup flow). Decision: delete in
> v2.0.5.

## Configuration and data on the PC

- `%AppData%\RSH\` — profiles, `settings.json`, logs
  (`logs\rsh-YYYYMMDD.log`, no dashes in the name).
- `%TEMP%\RSH\` — `startup-status.txt`, Serilog SelfLog, log fallback.
- The per-device key is stored in **DPAPI** (`KeyStore`), never in
  plaintext. The Wi-Fi password is read through the DPAPI path too;
  no plaintext copy is left in `settings.json`.
- Logs contain no secrets (verified by security re-scan).

## Build, test, publish

```powershell
# Build (needs .NET 10 SDK 10.0.100)
dotnet build software/RSH.slnx -c Release

# Tests (RSH.App.Tests is Windows-only; the rest run on Linux too)
dotnet test software/RSH.slnx -c Release

# Framework-dependent loose publish (what the release ships)
dotnet publish software/src/RSH.App -c Release -r win-x64 --self-contained false
```

The release is built by GitHub Actions (`.github/workflows/release.yml`)
on tag `v*`: firmware bins (PlatformIO, signed) + Windows app zip →
GitHub Release. CI (`.github/workflows/ci.yml`) runs build, tests,
host tests, firmware build and the benchmark gate on every push/PR.
See [`TESTING.md`](TESTING.md).
