# ApexLink — Final Delivery Report (Software v1.0.0)

Date: 2026-11-10 · By Dreamsofts · Companion to firmware v3 (DevKit V1, delivered separately)

## What you got

| Deliverable | Where |
|---|---|
| **ApexLink.exe** (portable, self-contained, single file — no .NET install needed) | `release/win-x64/ApexLink.exe` (195 MB — includes the .NET 10 runtime + WPF; runs on any Win10/11 x64) |
| Installer script (per-user, no admin) | `release-assets/ApexLink-Setup.iss` + `build-release.ps1` |
| User guide | `USER-GUIDE.md` (install → vJoy → wizard → calibrate → game → troubleshooting) |
| Source | `software/` (Core + VJoy output + WPF app + Simulator + Benchmarks + tests) |
| Firmware | `ApexLink-Firmware-DevKitV1-v3.zip` (already delivered) |

## The app (10 pages, all functional)

Dashboard (live gauge, pedal bars, link + output status, watchdog banner) ·
Input Monitor (raw values, button lamps, counters, 3-line live graph) ·
Calibration (centre + range capture for steering and pedals) ·
Processing (deadzone, curve with live preview drawn from the real curve
function, One Euro smoothing, inverts, combine-pedals) · Buttons (gear
mapping 1–128, digital pedal ramp, live test) · Profiles (create /
duplicate / delete / import / export, instant switching) · Output
(vJoy status with driver/DLL versions and axis/button facts, device
picker 1–16, joy.cpl + vJoyConf shortcuts) · Connection (ESP IP, HELO
state, remote firmware restart) · Diagnostics (link counters, engine
timing incl. avg/max/late ticks, this-app CPU/RAM, one-click diagnostics
.zip export) · Settings (theme System/Dark/Light, autostart, start
minimised, close-to-tray, log level) — plus a 5-step first-run wizard
(vJoy check → WiFi link → steering → pedals), system tray with output
toggle, single-instance guard, and crash-safe config (atomic save +
backup + defaults fallback).

## Verification done here (all real, this machine)

- Full solution build, Release: **0 warnings, 0 errors** (production code
  is warnings-as-errors).
- Unit/integration tests: **27 passed, 0 failed, 1 skipped** —
  Core 23 (protocol incl. the firmware golden packet cross-check, CRC,
  mapping/curve/filter/ramp math, profile crash-safety, engine watchdog
  and reconnect behaviour) + vJoy 4 (struct layout 124 bytes with exact
  field offsets vs the real driver header, neutral position, safe
  failure when the DLL is absent, guarded update). The 1 skip is the
  real-UDP loopback test: this build machine's sandbox forbids UDP
  loopback by policy; the identical logic is covered socket-free, and
  the test runs fully on Windows.
- Benchmarks (`tools/ApexLink.Benchmarks`, Release): packet parse
  **382 ns, 0.00 B allocated/packet** · processing pipeline **446 ns/tick,
  0.00 B/tick** · engine soak at exactly 200 ticks/s: max tick 637 µs,
  **0 late ticks, 0 Gen2 collections**, heap flat 0.4→0.6 MB, working set
  ~53 MB (bare console host, this VM). UDP end-to-end bench skipped here
  for the same sandbox policy; it runs on Windows.
- Firmware (from its own report): 536 host checks, 0 failures;
  independent Python packet verification; PlatformIO compile clean.

## Verified design facts (not guesses)

- vJoy: BrunnerInnovation **v2.2.2.0** (the jshafer 2.1.9.1 in the
  original docs is Windows-10-only per its own release notes). Driver
  and DLL must come from the same release — the app checks `DriverMatch`
  and says so in plain language.
- The P/Invoke struct was matched byte-for-byte against the driver's own
  `public.h`/`vjoyinterface.h` fetched from the vendor repo (API V3
  layout; our used fields sit at identical offsets in V2/V3 and the V3
  buffer is the larger one, so it is safe with either DLL). Axis range
  1–32768, centre 16384. `UpdateVJD` (one struct per tick) is the 200 Hz
  path; `RelinquishVJD` is void, as the header says.
- The app finds `vJoyInterface.dll` next to itself or in
  `C:\Program Files\vJoy\x64` automatically.

## ⚠ What only YOU can verify (Windows + real hardware)

These were impossible in this Linux build environment and are **not**
claimed as tested:

1. The app window itself (Mica/Fluent rendering, tray icon, wizard flow)
   — compiled clean against the real WPF-UI 4.3.0, never run.
2. Real vJoy driver behaviour (acquire/update on device 1, joy.cpl axes
   moving, HVCI/Memory-Integrity acceptance on your machine).
3. Real wheel end-to-end: ESP32 ↔ app at 200 pps on your WiFi, real
   calibration feel, One Euro tuning (starting values freq 200 /
   mincutoff 1.0 / beta 0.1 are the author's pattern, tuned-by-feel on
   hardware — the smoothing slider maps onto them).
4. A real game binding pass (any title) and the 8-hour soak budget on
   your PC (`ApexLink.Benchmarks` prints the numbers there in ~1 minute
   plus soak time).
5. The Inno installer — build it on Windows with `build-release.ps1`
   (needs Inno Setup installed); the portable exe needs nothing.

## Known honest limits

- If Windows Memory Integrity blocks the vJoy driver, no app can fix
  that from user space — the Output page surfaces the driver's state
  truthfully instead of pretending.
- While racing, the PC is on the wheel's WiFi (no internet on that
  adapter) — inherent to the SoftAP design in the project docs.
- EXE is large (195 MB) because it carries its own runtime — chosen so
  it runs on a clean Windows with zero installs beyond the vJoy driver.

---

## Addendum — deep audit + virtual user testing (2026-11-10, v1.0.0 audit build)

A full second pass over firmware + software (line-by-line re-read of the
protocol, input, network, engine, pipeline, filter and vJoy layers),
followed by a virtual user-testing suite that drives the engine exactly
as a user would — simulated wheel in, recorded joystick out.

**Bugs found and fixed in this pass:**
1. **Safety (software):** turning Output OFF left the last steering/pedal
   values latched in vJoy — the car in-game kept whatever the wheel last
   said. The engine now sends exactly one neutral frame when output is
   disabled. Covered by a regression scenario test.
2. **Robustness (software):** a UDP socket error in the receive loop
   could spin hot; it now backs off 100 ms.
3. **Latent overflow guard (firmware):** the steering sample count is now
   clamped to the sample buffer size, so a future config change cannot
   overrun it.
4. **UI correctness (software):** the Calibration page now reloads its
   values when the active profile changes instead of showing the
   previous profile's numbers.

**Virtual user tests (10 scenarios, all passing):** first drive
(centre/full-left/full-right exact vJoy values), pedals full range,
gear-button bit mapping, wheel-disappears watchdog → neutral + recovery,
output-off neutral safety, vJoy-busy engine stays alive + auto-reconnect,
packet-loss statistics accuracy, mid-drive profile switch (mirror),
digital pedal-button ramp timing, and a 30 s / 6000-tick soak with zero
lost ticks.

**Re-verified after the fixes:** full test suite 37 passed + 1 skipped
(the UDP-loopback test self-skips where the OS/sandbox forbids loopback;
its logic is covered socket-free), firmware host suite 536 checks /
0 failures (rebuilt from source), Python packet cross-verify PASS,
firmware PlatformIO esp32dev compile SUCCESS, full solution Release
build clean.

**Installer:** a compiled `ApexLink-Setup-1.0.0.exe` (NSIS, per-user,
LZMA) now ships alongside the portable exe; the Inno script and this
NSIS script both remain in `release-assets/` for rebuilds.
