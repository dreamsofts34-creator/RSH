# Changelog — RSH (Real Sim Hub)

All notable changes to the RSH app and firmware. Current versions:
**app v2.0.4 · firmware v4.0.2 · protocol v2.1 (frozen)**.
Release notes on GitHub are the same text, shorter; this file is the
full record. The C# series (v2.0.4 and earlier) is superseded — the
Rust series below starts at v1.0.0.

## [2.0.4] — 2026-11-19

Round 2 deep-scan fixes (51 findings: 2 P0, 24 P1, 25 P2), merged as
five workstreams (merge `332f341`, gate fixes `c9a1766`):

- **Game auto-scanner fully removed.** Games are now added manually
  (name + exe/shortcut) and launched locally. No more Steam/iRacing
  auto-detection, no registry scanning, no `reg.exe`/`tasklist` on
  any hot path.
- **Setup wizard fixed.** Step 1 board-connect check (Next unlocks
  when the board is reachable), Step 2 drivers check, Step 3
  pairing. Stored-key case now offers confirm-replacement instead of
  a dead end; the 10 s Begin-pairing wait works.
- **Connection state machine fixed.** The manager reaches `Active`
  for a paired board; `fail()` arms the backoff; retry works from
  `Error` instead of latching there.
- **Tray popup stays visible** (5 s minimum); tray registry spawn
  fixed; perf: firmware check cached, vJoy cached, `set_tuning` no
  longer triggers a full refresh; background resilience
  (power-throttling opt-out, power request while the wheel is
  Active).
- **UI/UX polish.** Spacing pass, live pedal graphs at 20 Hz,
  per-button busy states, loading and error states, theme handling.
- **Release:** NSIS installer + firmware v4.0.2 (merged full-flash
  image + Ed25519-signed OTA image). Three assets.

## [2.0.3] — 2026-11-18

- Deep-scan fixes (DS-01–DS-82): IPC pipe loop fixed, window-close
  crash (SIGSEGV) fixed, daemon respawn loop fixed, exit codes
  fixed, reconnect-failed spam stopped.
- vJoy is the default backend. First-run wizard (5 steps, drivers
  step). Tray + first-run wizard.
- **Release:** installer + firmware v4.0.2 (merged + signed OTA).
  Three assets.

## [2.0.2] — 2026-11-17

- First three-asset release (installer + merged firmware + signed
  OTA image for the app's WebOTA flow).
- Smart installer: detects an existing install and offers
  update / reinstall (with a downgrade warning).

## [2.0.1] — 2026-11-17

- Hotfix: daemon self-spawn + live IPC, fake sim data removed (real
  disconnected state), all buttons wired, tray + first-run wizard
  (5 steps, drivers step), vJoy default backend.
- **Release:** installer + firmware image. Two assets (OTA added in
  v2.0.2).

## [2.0.0] — 2026-11-16

- v2.0 redesign: Slint UI + always-on daemon (Tauri/React gone).
  Three nav entries (Overview, RSH Wheel, Settings), dark theme,
  920×580 window. Firmware v4.0.2, protocol v2.1 frozen.
- **Release:** installer only. Single asset.

## [1.0.2] — 2026-11-15

- Hotfix: startup failures are now visible (native error dialog +
  dual-location startup log); no console window on launch (app +
  sidecar are GUI-subsystem); launch path smoke-tested in CI.

## [1.0.1] — 2026-11-15

- Hotfix: no console window on launch (app + sidecar
  GUI-subsystem).

## [1.0.0] — 2026-11-15

- First Rust release: Tauri 2 app (NSIS + MSI) + C++ firmware
  v4.0.1 (signed). Full Rust recreation of the C# app; protocol
  v2.1 frozen; Ed25519-signed firmware.
