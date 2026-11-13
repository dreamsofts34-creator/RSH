# RSH (Real Sim Hub, formerly RSH) — Project Handover (updated)

**Product:** RSH — wireless DIY sim-racing steering wheel, by **Dreamsofts** (Ali hamza).
**Status at handover (2026-11-11, updated round):** Software **v1.2.2**, Firmware **v3.3**. Built and code-level tested; delivered via Google Drive. On-PC / on-board live verdict still pending from the owner (flashing was in progress — board connects and is detected correctly).

This package contains everything: all project documents, the flashing guide, the complete firmware source, the complete PC software source at v1.2.2, the owner's two UI-update patch zips, host test source, source backups of v1.2.0 and v1.2.1, and the ready release files.

---

## 1. What RSH is

An ESP32 board reads a steering potentiometer, two hall-effect pedals and four buttons, and streams the state over its own Wi-Fi access point (UDP) to a Windows companion app. The app calibrates and processes the signal (filtering, curves, deadzones) and feeds a **vJoy** virtual joystick, which any racing game can bind like a real wheel.

Design principle: **firmware dumb, PC smart.** All calibration and processing live on the PC; the board only samples, packs and sends. Calibration therefore survives firmware updates and never needs re-flashing to tweak.

## 2. System overview

```
[Steering pot] [Accel hall] [Brake hall] [4 buttons]
        \           |            |           /
              ESP32 DevKit V1  (firmware)
              Wi-Fi AP "RSH" · UDP 200 packets/s
                        |
              Windows PC (RSH app)
              calibration → filtering → curves
                        |
              vJoy virtual joystick → racing game
```

- PC joins the `RSH` Wi-Fi network, runs the app, runs the setup wizard once, then binds the vJoy device in the game.
- The app's wizard can join the wheel Wi-Fi itself ("Connect automatically"), open vJoy Configure, walk through firmware update, auto-calibration, button test and output test.

## 3. Hardware

- **Board:** ESP32 DevKit V1 (original ESP32, chip ESP32-D0WD-V3 as detected on the owner's board). The ESP32-C3 Super Mini was dropped: its ADC needed a series resistor the owner does not have; the DevKit V1 reads the 3.3 V steering pot directly. C3 firmware source is preserved in `firmware/rsh-c3/` for reference.
- **Pin map (DevKit V1):**
  - Steering potentiometer → **GPIO34**
  - Accelerator hall sensor → **GPIO35**
  - Brake hall sensor → **GPIO36**
  - Buttons → **GPIO25 / 26 / 27 / 33** (INPUT_PULLUP, wired to GND)
  - Status LED → **GPIO2** (active HIGH)
  - All analogue inputs are on ADC1 (the only ADC usable with Wi-Fi on).

## 4. Wire protocol (fixed, version 1)

18-byte little-endian UDP packet:

| Bytes | Field |
|---|---|
| 0 | Magic `0xA5` |
| 1 | Version `1` |
| 2–3 | Sequence (u16) |
| 4–7 | ESP timestamp (u32, µs) |
| 8–9 | Steering (u16) |
| 10–11 | Accelerator (u16) |
| 12–13 | Brake (u16) |
| 14 | Buttons bitmask (bit0 gear up, bit1 gear down, bit2 accel btn, bit3 brake btn) |
| 15 | Flags (bit0 = steering value is in mV) |
| 16–17 | CRC16-CCITT (poly 0x1021, init 0xFFFF) over bytes 0–15 |

- PC sends `HELO` to the board on port **4210** about once per second.
- Board unicasts data packets to the PC on port **4211** at **200 pps**.
- If no HELO arrives for 2000 ms, the board goes silent.
- Golden test packet: `a5 01 34 12 04 03 02 01 fc 08 e8 03 d0 07 05 01 75 bd` (cross-verified in Python and in the C# test suite).

## 5. Firmware (ESP32, Arduino framework via PlatformIO)

Source: `firmware/rsh/` (`rsh.ino`, `config.h`, `protocol.h`, `inputs`, `net`, `status_led`, `webota`).

Features (v3.3):
- Wi-Fi SoftAP `RSH` with automatic channel selection; up to 4 stations.
- Adaptive sampling governor, trimmed-mean steering read, mV steering mode.
- ArduinoOTA **and** a browser Web OTA page at `192.168.4.1` (HTTP Basic Auth, dual-partition safe, progress UI in the v3.3 light theme).
- Remote restart command, low-heap safety restart, status LED patterns.
- Passwords and SSID live in `config.h`. The delivered builds keep the factory defaults by the owner's explicit choice; change them in `config.h` before flashing for any non-personal use.

**Flashing / updating** — full steps in `docs/FLASH-GUIDE.md`.
- First flash: merged factory `.bin` at address `0x0` via the Adafruit web flasher (first/top file row, offset 0) or esptool.
- Later updates: upload the app `.bin` through the Web OTA page, or from the PC app's setup wizard (Firmware step). Dual-partition keeps the previous image as fallback on failure.
- Host-side logic tests: `tests-host/` (536 checks, plain C++17, no Arduino needed).

## 6. PC software (C# · .NET 10 LTS · WPF + WPF-UI 4.3)

Solution: `software/RSH.slnx` (source in this package is **v1.2.2**)

| Project | Purpose |
|---|---|
| `src/RSH.Core` | Protocol, UDP link, calibration, signal processing (One Euro filter, curves, deadzones), profiles, auto-calibrator. No UI, fully unit-tested. |
| `src/RSH.Output.VJoy` | vJoy feeder (P/Invoke, cdecl, axis range 1–32768, centre 16384). Requires the vJoy driver (BrunnerInnovation build, device enabled in vJoyConf with axes X/Y/Z). |
| `src/RSH.App` | WPF app: pages (Dashboard/Home, Connection, Calibration, Processing/Tune, Buttons, Profiles, Monitor, Output, Diagnostics, Settings, Setup), 7-step auto-setup wizard, tray icon, autostart, single-instance, per-game auto profile switching by foreground process. |
| `tools/RSH.Simulator` | Fake board for testing without hardware. |
| `tools/RSH.Benchmarks` | Pipeline benchmarks. |
| `tests/RSH.Tests` | Core test suite. |
| `tests/RSH.VJoy.Tests` | vJoy interop tests. |

Design history: v1.2.0 introduced the light pastel theme (white rounded cards, violet accent `#6C4CF1`, minimal one-line professional-English copy). The owner then supplied two UI-update patches (authored with another Claude) that were applied and shipped as v1.2.1 (Setup page, Home/Setup/Tune) and v1.2.2 (new sidebar design, `Themes/Theme.xaml`). The patch zips are in `patches/`; pre-patch sources are in `backups/`.

**Build & run**
- Needs the .NET 10 SDK. `dotnet build software/RSH.slnx -c Release`, tests via `dotnet test`.
- Shipping build is the framework-dependent "LITE" publish (win-x64, ~28 MB folder): `dotnet publish src/RSH.App -c Release -r win-x64 --self-contained false`. The target PC needs the one-time **.NET 10 Desktop Runtime**.
- No installer ships by the owner's decision (download size); the essentials zip is the distribution format.

## 7. Versions & delivery history

- **Firmware v3.1 / Software v1.0.0** — first complete delivery (2026-11-10/11).
- **Firmware v3.2 / Software v1.1.0** — audit round 2 (30+ fixes), advanced auto-setup wizard, per-game profiles, rotation presets.
- **Firmware v3.3 / Software v1.2.0** — design round: light theme per the owner's reference image, minimal copy, auto-setup that acts (self Wi-Fi connect, vJoy Configure launcher), Web OTA page restyled.
- **Software v1.2.1** — owner's UI Update v1 patch applied (Setup page, Home/Setup/Tune). Build 0/0, tests 42 passed + 1 skipped.
- **Software v1.2.2** — owner's UI Update v2 patch applied (new sidebar design). Build 0/0, tests 42 passed + 1 skipped. Release: `RSH-Essentials-v1.2.2.zip` (in `release/`).

Full detail: `docs/DELIVERY-NOTE.md`, `docs/FINAL-REPORT.md`, `docs/TEST-REPORT.md`, `docs/USER-GUIDE.md`, `docs/PROJECT.md`, `docs/SOFTWARE-PLAN.md`, and the original spec set in `docs/sim-wheel-docs/`.

## 8. Verification status (honest)

Verified in the build environment:
- Software tests: **42 passed, 1 skipped** (the skipped one needs real UDP loopback, blocked in the sandbox). Release builds for v1.2.0–v1.2.2: **0 errors, 0 warnings**. The v1.2.0 app folder was re-verified byte-identical to a fresh build from source.
- Firmware host tests: **536 checks, 0 failures**; packet format cross-verified; PlatformIO compile for esp32dev: **SUCCESS**.

Not verifiable there — pending on the owner's Windows PC / real board:
- App look & feel on Windows (all design rounds), Wi-Fi auto-connect on the real network, vJoy Configure button, calibration feel, game binding.
- Flash completion (board detected and guided on 2026-11-11; final result not yet reported), board debug numbers (cycle timing, ADC noise with Wi-Fi on, LED polarity), OTA update in practice.

## 9. Delivery-channel note (2026-11-11)

Chat and Gmail both blocked the release zips (Gmail blocks any archive containing an `.exe`/`.dll` by policy; the app is unsigned, which makes filters stricter). **This is a policy block, not a virus finding** — the shipped files were proven byte-identical to a fresh build from this source. The working route is Google Drive: files uploaded to Drive, reader access shared to the owner's Gmail, and the Drive link sent by email. A final on-PC check the owner can run any time: right-click the extracted folder → **Scan with Microsoft Defender**.

## 10. v2 direction (planned, nothing built yet)

Product renamed **RSH — Real Sim Hub**. Universal modular wheel: everything on **one ESP32**, user attaches only the modules they want, pins assigned from the software, any power source (battery/SMPS/USB). Plans:

- `docs/FIRMWARE-PLAN.md` — runtime pin config, protocol v2, control channel, safety/rollback, phases F0–F6
- `docs/SOFTWARE-PLAN-v2.md` — per-module Tune, add-module/pin UI, game launcher + per-game profiles, WPF re-skin, phases S0–S6
- `docs/ui-reference/rsh-ui-preview.html` — owner's UI preview (visual spec only; known bugs listed in the software plan §4.1)

Non-negotiable: v1.2.2 performance must not regress (software plan §0, firmware plan §0).

**S0/F0 status: DONE (planning only).** Protocol v2 contract frozen (`docs/PROTOCOL-V2.md`, golden vectors + reference encoder in `tests-host/protocol_v2_vectors.py`); pin capability table + reference validator, 14 checks pass (`docs/board-esp32-devkitv1.json`, `tests-host/pin_rules_ref.py`). All open decisions were closed by delegation (see decision tables in both plans). Numeric perf baseline is **optional** (`docs/PERF-BASELINE.md`, `tools-perf/`) because it cannot be measured; no-regression is enforced by construction (plans §0). **No v2 code exists yet** — start only on "start S1 / start F1".

## 11. Package contents (cleaned, source-only)

```
HANDOVER.md                  <- this file
docs/                        FIRMWARE-PLAN, SOFTWARE-PLAN-v2, PROJECT, SOFTWARE-PLAN, DELIVERY-NOTE,
                             FINAL-REPORT, TEST-REPORT, USER-GUIDE, FLASH-GUIDE,
                             ui-reference/ (UI preview), sim-wheel-docs/ (original specs)
firmware/rsh/           DevKit V1 firmware source (v3.3) + platformio.ini + merge_bin.py (merged 0x0 image)
software/                    full C# solution source (v1.2.2), no bin/obj
tests-host/                  firmware host tests (test_main.cpp, run_tests.sh, verify_packet.py) + protocol_v2_vectors.py, pin_rules_ref.py
tools-perf/                  Measure-RSH.ps1 (app perf baseline script)
```

Removed from the original handover (rebuild or ignore): `release/` (firmware .bin + Essentials zip), `backups/` (v1.2.0 / v1.2.1 source), `patches/` (UI-update zips, already applied in v1.2.2), `firmware/rsh-c3/`, `firmware/step0-ap-test/`, and compiled `tests-host/run_tests*` binaries. Older docs under `docs/` may still mention those paths — historical.
