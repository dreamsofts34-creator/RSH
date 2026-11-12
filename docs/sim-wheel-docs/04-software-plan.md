# 04 - PC Software Plan (C# .NET 10, WPF, vJoy)

## Goals

Modern GUI, saari calibration/settings software mein, Windows ke saath start, stable 24x7, firmware dobara flash kiye bina tuning.

## Tech

- .NET 10 LTS, WPF, **WPF-UI 4.x** (Fluent, Mica, dark/light, NavigationView)
- CommunityToolkit.Mvvm, Microsoft.Extensions.Hosting (DI), Serilog, ScottPlot 5, xUnit
- Publish: `dotnet publish -c Release -r win-x64 --self-contained -p:PublishSingleFile=true`
- Installer: Inno Setup (shortcut, autostart option, vJoy check)
- IDE: Visual Studio (latest) ya Rider, .NET 10 SDK

## Modules (Core)

| Module | Kaam |
|---|---|
| `Protocol` | 18-byte parse/validate, CRC16, wrap-safe sequence |
| `Transport` | `IPacketSource`: `UdpSource` (pehle), `SerialSource` (baad mein), `SimulatorSource` |
| `Calibration` | steering left/center/right, pedal min/max, validation |
| `Processing` | deadzone, curve, One Euro filter, ramp (button pedals), steering lock scaling |
| `Profiles` | JSON profiles, schema version + migration, atomic save, backup |
| `Output` | `IOutputBackend`; `VJoyBackend` (P/Invoke) |
| `Diagnostics` | pps, loss %, inter-arrival p50/p99, bad CRC, latency graph data |

## vJoy backend

- vJoy ki `vJoyInterface.dll` **install folder se load** karo (`NativeLibrary.SetDllImportResolver`), bundle mat karo: driver ke saath hamesha match.
- Start par: `vJoyEnabled`, `GetvJoyVersion`, `DriverMatch` check; mismatch par UI warning.
- `AcquireVJD(id)`, har update par `SetAxis` x3 + buttons, phir `UpdateVJD` (ya `UpdateVJD` ke saath position struct). Exit par `RelinquishVJD`.
- Axis range ~0..32768, steering ke liye poora istemal.
- Device status check: FREE/OWN/BUSY/MISS ke alag error messages.
- Recommended driver: **vJoy 2.1.9.1** (Win11 par kaam karta bataya gaya, signed). 2.2.x (njz3/Brunner) sirf zaroorat par.

## Data path rules (reliability)

- Dedicated UDP thread, blocking receive, reused `byte[]`/`Span<byte>`, koi per-packet allocation nahi.
- Latest-state single slot; processing usi thread ya fixed-rate loop mein.
- Watchdog: 100 ms par neutral. Reconnect par filters/ramp reset.
- UI dispatcher se data path kabhi block nahi hota (UI sirf 30-60 Hz snapshot padhta hai).
- Unhandled exception handler: log + axes neutral + relinquish.
- Single instance (named mutex).

## GUI pages (WPF-UI NavigationView)

1. **Dashboard**: connection badge, pps/loss/latency graph, live bars (steering, accel, brake, buttons), "Output: vJoy OK".
2. **Calibration**: guided wizard (left/center/right, pedals), live raw values, validation warnings.
3. **Steering**: steering lock (180-900°), curve editor (expo + custom points), deadzone, smoothing (One Euro: min cutoff, beta), invert, center offset, live preview.
4. **Pedals**: har pedal ka mode (button/analog), curve, deadzone, ramp up/down (button mode), invert, min/max.
5. **Buttons**: mapping to vJoy buttons, min hold time, test.
6. **Output**: vJoy device ID, axis mapping, test sweep, driver version info.
7. **Profiles**: per-game profiles, duplicate/import/export, optional auto-switch by game process name.
8. **Network**: ESP IP, ports, status, signal hint, HELO interval.
9. **Diagnostics**: live log, packet stats, record/replay capture.
10. **Settings**: start with Windows, start minimized to tray, theme, language, check for updates.

## Tray + autostart

- WPF-UI NotifyIcon: connection state icon (grey/amber/green), menu: Open, profile switch, Pause output, Exit.
- Close button = tray mein minimize (setting).
- Autostart: `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` ya Task Scheduler (logon). Default minimized.

## Processing details

Steering: raw -> clamp [left,right] -> normalize -1..1 (left/right alag scale) -> deadzone -> curve `sign(x)*|x|^exp` (ya spline) -> One Euro filter -> lock scaling -> vJoy X.

Pedals analog: normalize -> invert -> deadzone -> curve -> filter. Pedals button: `v += dt/ramp_up` pressed, `v -= dt/ramp_down` released, clamp 0..1 (`dt` Stopwatch se).

Gears: edge latch, `min_hold_ms` (default 40).

## Profile JSON (example)

```json
{
  "schemaVersion": 1,
  "name": "Default",
  "steering": { "left": 420, "center": 1640, "right": 2860, "invert": false,
    "lockDegrees": 900, "deadzone": 0.01, "curveExp": 1.0,
    "filter": { "type": "oneEuro", "minCutoff": 1.0, "beta": 0.02 } },
  "accelerator": { "mode": "button", "min": 0, "max": 4095, "invert": false,
    "rampUpS": 0.3, "rampDownS": 0.2, "deadzone": 0.02, "curveExp": 1.0, "vjoyAxis": "Y" },
  "brake": { "mode": "button", "min": 0, "max": 4095, "invert": false,
    "rampUpS": 0.2, "rampDownS": 0.15, "deadzone": 0.02, "curveExp": 1.0, "vjoyAxis": "Z" },
  "buttons": { "gearUp": 1, "gearDown": 2, "minHoldMs": 40 },
  "network": { "espIp": "192.168.4.1", "helloPort": 4210, "dataPort": 4211 },
  "timeoutMs": 100,
  "output": { "backend": "vjoy", "deviceId": 1 }
}
```

Config path: `%APPDATA%\SimWheel\` (profiles/, settings.json, logs/). Save = temp file + rename + .bak.

## Testing

- Unit tests: CRC, wrap-safe sequence, calibration math, curves, ramp, filter.
- Replay tests: recorded packet capture -> expected output.
- **Simulator tool**: fake ESP32 jo sine-wave steering/pedals bhejta hai, packet loss/jitter inject kar sakta hai; hardware ke bina pura app test.
- Soak: 24 ghante simulator ke saath, memory/handle leak check.

## Milestones

1. Core: protocol + CRC + tests, Simulator
2. UdpSource + diagnostics CLI (console harness)
3. VJoyBackend + manual axis test
4. Processing (steering) + calibration wizard
5. Pedals (button ramp, analog) + gears
6. WPF shell: Dashboard, Calibration, Steering, Pedals
7. Profiles, Output, Network, Diagnostics, Settings
8. Tray + autostart + single instance + crash handling
9. Installer, signing (optional), docs
