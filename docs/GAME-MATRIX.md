# RSH — Game Compatibility Matrix (Rust recreation, Phase G draft)

**Phase H final (2026-11-14, H5):** re-checked — **still 0 of 16
backend–game pairs verified in-game.** Nothing in this file was upgraded:
no game, on either backend, has passed the in-game verification procedure
below on the Rust build. In-game verification remains a Windows-only
activity, pending on a real Windows PC with the real game. This file is
final for v1.0.0 as an honest 0/16 record; verification results land here
only after the procedure has actually run, with date + build + backend.

**Status: ALL ENTRIES UNVERIFIED — every status below is "Windows-only /
not yet measured", on BOTH backends.** No game in this matrix is
"verified", "supported" or "works" on the Rust build — on HIDMaestro or
on vJoy — until the verification procedure (§ Verification procedure)
has been run on a real Windows PC with the real game, on that backend,
and the result recorded here. This file is the Phase G update of the
Phase F draft; the Phase G change is per-backend status (one column per
output backend, SW-R6.5): a game verified on vJoy is NOT verified on
HIDMaestro, and vice versa. Nothing in it upgrades a status on the
strength of code reading or sandbox tests alone.

**Build under test (when verification runs):** Rust app v1.0.0 (Tauri 2) +
C++ firmware v4.0.1, protocol v2.1, Windows 11 x64, output backend
**HIDMaestro** (default, Phase G2 — elevated sidecar, UAC prompt on
launch) **or vJoy** (fallback, Phase G3 — Rust FFI, no UAC prompt).
FFB is out of scope in all cases: the wheel has no force-feedback motor.

## What was actually measured in this sandbox (Linux VM, 2026-11-14)

These are code-level checks only. **None of them verifies any game.**

- **Launcher catalogue (SW-R7.3, `crates/rsh-app-core/src/launcher.rs`):**
  detection of Assetto Corsa + DiRT Rally 2.0 from fixture Steam libraries
  (`libraryfolders.vdf` parse, exe lookup under `steamapps/common/`),
  launch tracking (one game at a time), stop kills only the tracked pid
  (SEC-E-10), refresh clears a game that exited on its own — all with a
  fake process spawner in `cargo test`. The real Windows spawn path
  (`SystemSpawner`) was **not** exercised here.
- **Auto-switch (F2, `crates/rsh-profiles/src/resolver.rs` +
  `watcher.rs`):** switch scenarios with stub car/foreground sources —
  game detected → game profile, car stable 4 polls / 2 s → car profile,
  failed switch → profile unchanged, game closed → pre-game profile
  restored, 400 ms foreground debounce. All with stubs, no game.
- **Output backends (Phase G, sandbox only):** the selector
  (`crates/rsh-output/src/selector.rs`) defaults to HIDMaestro with a
  vJoy fallback and records the fallback reason; the vJoy backend
  (`crates/rsh-output/src/vjoy.rs`, 78 tests) and the HIDMaestro
  backend (`crates/rsh-output-hidmaestro/`, 118 tests) both pass their
  unit suites on Linux with mock drivers/sidecars. The engine-spawn
  wiring (`crates/rsh-app-core/src/state.rs`) starts the engine task
  at launch, activates the persisted backend choice (default
  HIDMaestro, fallback vJoy, reason recorded), and feeds loopback UDP
  frames end-to-end into a recording backend in `cargo test`. **None
  of this verifies any game, any real driver, or any real board.**
- **Not measured here:** any game detecting the wheel, any axis/button
  responding in a game, any telemetry reader output, any anti-cheat
  behaviour, any 10-minute stability run, any real HIDMaestro sidecar
  or vJoy driver on Windows. All of that is Windows-only (see per-game
  "Still needed" rows).

## RSH input chain (design, not a verified fact)

- The wheel (ESP32) sends input over Wi-Fi UDP (protocol v2.1); the PC
  app presents it to the game as a DirectInput device via HIDMaestro
  (default) or vJoy (fallback). The game never sees the wheel directly.
  **Each backend presents a different device to the game** (SW-R6.5):
  a game that has seen the vJoy wheel sees the HIDMaestro wheel as a
  brand-new device, and its bindings must be made again from scratch.
- **XInput-only titles will not see an RSH wheel on either backend.**
  A title that only reads XInput (Xbox-style controller) input shows
  no RSH wheel; that is a "not detected — XInput-only" result, not a
  bug.
- **No FFB:** the wheel has no force-feedback motor, so no game's FFB
  features work on RSH. "FFB does not work" is the expected result for
  every game below, never a failure.
- **Rate note:** RSH input runs at 200 Hz. Do not compare it with a
  game's FFB output rate (e.g. iRacing's 360 Hz FFB output) — one is
  input, the other is FFB output. Games read a DirectInput device at
  their own physics rate (typically 60 Hz).
- **Handbrake:** mapped to the Slider axis (WSlider, HID usage `0x36`)
  on either backend (analog). vJoy has no dedicated handbrake axis;
  vJoyConf must have Slider enabled on device 1.
- **Anti-cheat:** the app injects nothing into any game (no
  `CreateRemoteThread` / `WriteProcessMemory` / hooks; the launcher only
  spawns and terminates the tracked process). That is a design statement
  from code inspection, not a measured fact — each game's anti-cheat
  behaviour still has to be observed on Windows (procedure step 9).
- **Platform:** Windows 11 x64; ARM64 explicitly unsupported. If
  neither backend is available (no HIDMaestro sidecar and no vJoy
  driver), output is disabled (Dashboard warns, app keeps running).
  HIDMaestro activation asks for elevation (UAC) on every launch
  while it is the active backend; vJoy needs no UAC prompt.

## Matrix

Per-backend status: each game is verified separately on HIDMaestro
(default) and on vJoy (fallback) — 16 backend–game pairs in total.
A pair is "NOT VERIFIED" until the procedure has been run for that
game **on that backend**.

| Game | Input API (expected) | HIDMaestro (default) | vJoy (fallback) | Launcher catalogue | Telemetry reader (per-car switch) | Still needed on a Windows PC |
|---|---|---|---|---|---|---|
| Assetto Corsa | DirectInput | **NOT VERIFIED — Windows-only** | **NOT VERIFIED — Windows-only** | Yes (`assettocorsa.exe`, Steam 244210; helper `acs.exe`) | F1 reader (AC shared memory) — Rust reader not yet verified on a live game | Procedure steps 1–9, per backend; helper-process stop (`acs.exe`) behaviour on exit |
| Assetto Corsa Competizione (ACC) | DirectInput | **NOT VERIFIED — Windows-only** | **NOT VERIFIED — Windows-only** | Yes (`acc.exe`, Steam 805550) | F1 reader (ACC shared memory) — Rust reader not yet verified on a live game | Procedure steps 1–9, per backend |
| iRacing | DirectInput | **NOT VERIFIED — Windows-only** | **NOT VERIFIED — Windows-only** | Yes (`iRacingSim64DX11.exe`, not on Steam — default install path) | F1 reader (iRacing shared memory) — Rust reader not yet verified on a live game | Procedure steps 1–9, per backend; Steam detection does not apply — verify the default-path detection and the iRacing launcher flow |
| rFactor 2 | DirectInput | **NOT VERIFIED — Windows-only** | **NOT VERIFIED — Windows-only** | No — Steam/exe paths unverified, add only after a Windows check | None — no rF2 telemetry reader in scope; per-car switch unavailable, game profile only | Procedure steps 1–9, per backend; verify Steam app id + exe + install dir before adding to the catalogue |
| Automobilista 2 | DirectInput | **NOT VERIFIED — Windows-only** | **NOT VERIFIED — Windows-only** | No — Steam/exe paths unverified, add only after a Windows check | None — no AMS2 telemetry reader in scope; per-car switch unavailable, game profile only | Procedure steps 1–9, per backend; verify Steam app id + exe + install dir before adding to the catalogue |
| DiRT Rally 2.0 | DirectInput | **NOT VERIFIED — Windows-only** | **NOT VERIFIED — Windows-only** | Yes (`dirt2.exe`, Steam 690790) | None — no DR2 telemetry reader in scope; per-car switch unavailable, game profile only | Procedure steps 1–9, per backend |
| F1 (series) | DirectInput — expected | **NOT VERIFIED — Windows-only** | **NOT VERIFIED — Windows-only** | No — one exe per title, each unverified; add only after a per-title Windows check | None — no F1 telemetry reader in scope; per-car switch unavailable, game profile only | Procedure steps 1–9 **per title, per backend** (titles differ); verify each title's exe separately |
| Forza (series) | DirectInput — expected; some titles may be XInput-only | **NOT VERIFIED — Windows-only** | **NOT VERIFIED — Windows-only** | No — Forza comes from the Microsoft Store / Xbox app, not Steam; the Steam launch path does not apply | None — no Forza telemetry reader in scope; per-car switch unavailable, game profile only | Procedure steps 1–9 **per title, per backend**; verify the Store/Xbox launch path separately; an XInput-only title records "not detected — XInput-only", not a bug |

**Games:** 8 entries (F1 and Forza are series — every title is verified
separately). **Verified in-game on the Rust build: 0 of 8 on HIDMaestro,
0 of 8 on vJoy (0 of 16 backend–game pairs).**

## Verification procedure (per game, real build, Windows 11 x64)

Run on the Rust app v1.0.0 + firmware v4.0.1 with one output backend
at a time (HIDMaestro first, then vJoy — a game that has seen one
backend sees the other as a brand-new device, so bindings are made
again from scratch), and record the result in the table above (only
then does that backend's status change from "NOT VERIFIED —
Windows-only" to "VERIFIED (date, build, backend)"):

1. **Detection:** launch the game — does the RSH wheel appear as a
   DirectInput device on the game's controls/settings screen?
   (XInput-only title → record "not detected — XInput-only", not a bug.)
2. **Steering:** full lock-to-lock — does the game steer in the correct
   direction and range?
3. **Pedals:** accelerator, brake, clutch (and handbrake where
   configured) — does each axis read 0–100% correctly?
4. **Buttons:** do the configured buttons bind and work in the game?
5. **Encoders:** where configured, do encoders produce input in the game?
6. **Stability / idle:** 10-minute session — does the device stay
   present (keepalive check)? Record any drop.
7. **Restore-on-exit:** on game exit, does the pre-game profile return
   (launcher restore, A2) — and does the watcher restore agree with it?
8. **FFB expectation:** FFB does not work — expected (no motor). Record
   "expected — no FFB", never a failure.
9. **Anti-cheat:** where the game has anti-cheat, any warning or block
   during launch or the session? Record the exact message.

**Recording rule:** a backend–game pair's status changes only after
this procedure has been run for that game on that backend (that title,
for F1/Forza) and the result written here with date + build + backend.
Unverified entries are never written up as "works". A game verified
on one backend is never written up as verified on the other.
