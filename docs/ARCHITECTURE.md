# RSH — System Architecture

> **Versions:** App **v2.0.4** · Firmware **v4.0.1** · Protocol **v2.1**
> (2026-11-14). This is the entry point to the architecture documentation.
> The full wire contract is [`PROTOCOL-V2.1.md`](PROTOCOL-V2.1.md);
> the development plan (v2.3) is the project authority for decisions.

## What RSH is

RSH — **Real Sim Hub** — is a DIY sim-racing steering wheel system:

- An **ESP32 DevKit V1** board reads the wheel hardware (steering
  potentiometer, hall-effect pedals, buttons, encoders, LED strip) and
  streams its state to a Windows PC.
- A **Windows companion app (C# · .NET 10 · WPF)** calibrates and
  processes the signal (filtering, curves, deadzones) and feeds a
  **vJoy virtual joystick**, which any racing game can bind like a
  real wheel.
- The wheel is **modular**: the user attaches only the modules they
  want, assigns pins from the app, and tunes each module separately.
  Power source (USB / battery / SMPS) is irrelevant to the app.

## Design principle: firmware dumb, PC smart

All calibration and processing live on the PC; the board only samples,
packs and sends. Calibration therefore survives firmware updates and
never needs re-flashing to tweak. The firmware does no curve fitting,
no filtering decisions, and no game logic — it reports raw-ish inputs
(mV / counts / cumulative encoder deltas) and the PC decides what they
mean.

## System overview

```mermaid
flowchart LR
    subgraph Wheel["ESP32 DevKit V1 — firmware v4.0.1"]
        S[Steering pot\nGPIO34] --> FW[200 Hz loop\nsample → pack → send]
        A[Accel hall\nGPIO35] --> FW
        B[Brake hall\nGPIO36] --> FW
        BTN[Buttons\nGPIO25/26/...] --> FW
        ENC[Encoders\nAS5600 / quadrature] --> FW
        LED[WS2812 strip\nGPIO21] --> FW
    end
    FW -- "Wi-Fi UDP 200 pps\nor USB-serial 921600 baud" --> PC
    subgraph PC["Windows PC — app v2.0.4"]
        PC --> ENG[WheelEngine\n200 Hz loop]
        ENG --> PIPE[ProcessingPipeline\nmap → deadzone → curve\n→ One Euro → pedal ramp]
        PIPE --> OUT[vJoy virtual joystick\ndefault backend]
        PIPE -.-> HID[HIDMaestro\n experimental backend]
        ENG --> UI[WPF UI — 8 screens\n30 Hz snapshot]
    end
    OUT --> GAME[Racing game\nAC / ACC / iRacing / ...]
```

## The two 200 Hz loops

There are two independent 200 Hz loops, one on each side of the link:

| Loop | Where | Period | Job |
|---|---|---|---|
| Sampling loop | Firmware (`rsh.ino`, Core 1) | 5 ms | Read ADC/buttons/encoders → pack a protocol v2.1 stream packet → send |
| Engine loop | PC (`RSH.Core/Pipeline/WheelEngine.cs`) | 5 ms | Take the latest packet → `ProcessingPipeline` → output to vJoy |

The loops are **not** synchronized with each other. The PC engine
always consumes the *latest* packet (event-driven, no polling) and a
**250 ms watchdog** sends NEUTRAL to the output if data goes stale.
The UI reads an engine snapshot at **30 Hz** — the engine never calls
into the UI.

Measured performance (v2.0.4 gate, Virtual Test Rig): packet parse
~205 ns, pipeline ~1.1 µs/tick, 0 B/tick allocation on the hot path,
end-to-end (NTP-style ping) p99 ≈ 4.8 ms. See
[`TESTING.md`](TESTING.md) and [`E2E-BUDGET.md`](E2E-BUDGET.md).

## Transports

Two transports carry the same protocol, selectable in the app
(Settings → Connection):

| Transport | Link | Notes |
|---|---|---|
| **Wi-Fi UDP** (default) | Board SoftAP `RSH` (`192.168.4.1`, `rsh.local`), PC joins the AP | HELO on port **4210**, stream on port **4211**, control on **4212**. Up to 4 stations. |
| **Wired USB-serial** | USB cable, 921600 baud | `RSHS` framed packets; auto-detect COM port; DTR/RTS must not reset the board. |

Both transports speak protocol v2.1; the engine does not care which
one is active. See [`PROTOCOL.md`](PROTOCOL.md).

## Configuration: stage → commit → confirm → rollback

Pin assignment and module configuration are **not** compile-time
constants anymore. The PC sends a config blob over the authenticated
control channel; the board stages it, commits it to NVS, and the PC
must confirm within **30 s** — otherwise the board rolls back to the
last-good config. A bad pin map can therefore never brick the wheel's
configuration. Details: [`SAFE-MODE-CONFIG-TXN.md`](SAFE-MODE-CONFIG-TXN.md),
[`PIN-MAP-PUSH.md`](PIN-MAP-PUSH.md), [`PIN-RULES.md`](PIN-RULES.md).

## Modules

Ten module kinds, each independently enabled/disabled and tuned
(`RSH.Core/Pipeline/ModuleKind.cs`):

Steering · Accelerator · Brake · Clutch · Handbrake · Shifter · Horn ·
Buttons · Encoders · LED

Limits (plan v2.3 D6): axes ≤ 8, buttons ≤ 128, encoders ≤ 4,
LED strips ≤ 1 (≤ 60 LEDs), batteries ≤ 1. Only **ADC1** pins
(32, 33, 34, 35, 36, 39) can be analogue inputs while Wi-Fi is on.
Default pin map: steering GPIO34, accelerator GPIO35, brake GPIO36,
buttons GPIO25/26, onboard LED GPIO2, WS2812 strip GPIO21.
See [`FIRMWARE.md`](FIRMWARE.md).

## Security model (summary)

- **Control channel:** HMAC-SHA256 authenticated frames, per-device
  key, nonce + counter (replay protection). Unauthenticated commands
  (e.g. the old `RSTX` restart) were removed.
- **Per-device key:** random 32-byte key generated on first boot,
  exchanged only inside a **60 s pairing window**; stored in DPAPI on
  the PC and in NVS on the board. The key survives factory reset.
- **Signed firmware:** Ed25519 signature, verified **before** apply,
  with anti-downgrade. The app also verifies the firmware signature
  itself (NSec.Cryptography) before upload.
- **Web OTA:** HTTP Basic Auth on every handler, lockout, CSRF
  protection. ArduinoOTA is disabled.
- **App:** unsigned for now (SmartScreen warning is expected);
  firmware signing is unaffected by that.

Full model: [`SECURITY.md`](SECURITY.md),
[`THREAT-MODEL.md`](THREAT-MODEL.md),
[`KEY-LIFECYCLE.md`](KEY-LIFECYCLE.md).

## Repository layout

```
README.md                  <- this project (start here)
docs/                      <- all documentation (index: docs/README.md)
firmware/rsh/              <- ESP32 firmware (PlatformIO, env esp32dev)
software/
  RSH.slnx                 <- solution
  src/RSH.Core/            <- protocol, engine, pipeline, profiles (no UI)
  src/RSH.App/             <- WPF app (8 screens, wizard, tray)
  src/RSH.Output.VJoy/     <- vJoy backend (default)
  src/RSH.Output.HIDMaestro/ <- HIDMaestro backend (experimental)
  src/RSH.RenderTest/      <- minimal plain-Window render test
  tests/                   <- RSH.Tests, RSH.VJoy.Tests, RSH.HIDMaestro.Tests, RSH.App.Tests
  tools/                   <- RSH.Simulator, RSH.Benchmarks
  packaging/               <- MSIX, winget, firewall, SBOM
tests-host/                <- firmware host tests (C++17, no Arduino needed)
tools-perf/                <- Measure-RSH.ps1 (perf baseline script)
.github/workflows/         <- ci.yml (per push/PR), release.yml (tag v* → GitHub Release)
```

## History (short)

- **v1.x (ApexLink, 2026-11-10–11):** first complete system — firmware
  v3.3 / app v1.2.2, protocol v1 (18-byte packet), fixed pin map.
  Delivered via Google Drive (chat/Gmail blocked release zips).
  The owner tested v1.2.2 on their PC and it ran.
- **v2.0.0 (2026-11-13):** rename to RSH, protocol v2.1, per-module
  tuning, control channel, signed OTA, wired serial, Virtual Test Rig.
  Phases A–H built in order with gates.
- **v2.0.1–v2.0.3:** hotfixes for a blank-window bug on the user's
  Windows 10 PC (startup logging, single-instance, Win10 fallbacks).
- **v2.0.4 (2026-11-14):** final polish sweep — the blank window's
  real root cause (the "classic" window was still a `FluentWindow`
  under the hood) fixed with a real plain `System.Windows.Window`
  (`ClassicMainWindow`) on Windows 10; firmware v4.0.1 (config-blob
  PIN-RULES validation, serial ping magic check, version reporting).

The original v1.2.2 handover package is preserved outside this repo
and is not part of it. Old planning documents in `docs/` that are
superseded say so in their own header — treat them as history, not
authority.

## Where to go next

- Firmware internals → [`FIRMWARE.md`](FIRMWARE.md)
- PC app internals → [`SOFTWARE.md`](SOFTWARE.md)
- Wire protocol (readable summary) → [`PROTOCOL.md`](PROTOCOL.md)
- Security → [`SECURITY.md`](SECURITY.md)
- Tests and gates → [`TESTING.md`](TESTING.md)
- User guide → [`USER-GUIDE.md`](USER-GUIDE.md)
- Flashing the board → [`FLASH-GUIDE.md`](FLASH-GUIDE.md)
