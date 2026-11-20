# Changelog — RSH (Real Sim Hub)

All notable changes to the RSH app and firmware. Current versions:
**app v2.0.6 · firmware v4.0.3 · protocol v2.2 (frozen)**.
Release notes on GitHub are the same text, shorter; this file is the
full record. The C# series (v2.0.4 and earlier) is superseded — the
Rust series below starts at v1.0.0.

## [2.0.6] — 2026-11-20 (in progress, not released)

**Pairing abolished — connect-play instead (feature, not a bugfix).**
User order 2026-11-20: the pairing system is removed outright and
replaced by a simple connect-play system. No release yet; work is
in progress on the `no-pairing-*` branches.

- **No device key, no HMAC, no pairing window.** Protocol v2.1 →
  v2.2 (version byte `0x22`): control frames are plain
  (`"RSHC" | cmd | reqId | len | payload | CRC16`); duplicates are
  caught by reqId only (last 32); `BAD_TAG` deleted; `KEY_RESET`
  opcode 0x0B deleted. HELLO stays 92 B — its key/password/nonce
  fields are reserved/zero and never disclose a secret.
- **Connect-play.** The board opens an open hotspot `RSH-XXXX`
  (last 4 hex of the board Wi-Fi MAC); the PC joins it (or USB),
  the app connects. The user sets a Wi-Fi password in
  **Settings → Connection** (8–63 printable ASCII); later
  connections use it. `SET_PASSWORD` carries the current password
  when one is set. Forgot it: hold BOOT ≥ 10 s — the firmware
  clears the password itself and reboots to an open AP (no wire
  command, no armed window).
- **WebOTA disabled until a password is set.** WebOTA auth is the
  user-set AP password (one credential); per-IP rate limit +
  Ed25519 verify + anti-downgrade kept.
- **Wizard is 4 steps** (Connect → Drivers → Calibrate → Games);
  Step 1 Next unlocks when the board is reachable (HELLO or
  better), never on pairing. Connection state machine is
  `Idle → LinkUp → Hello → Active`; `Active` = unsigned `GET_CAPS`
  OK. `board_reachable` latch drives the wizard and the banner.
- **Migration:** update the app first (v2.0.6), then flash firmware
  4.0.3 via WebOTA with the old stored Wi-Fi password; 4.0.3 first
  boot deletes the old NVS secrets (`devkey` / `paired` / `wifipw2`
  / `wifipw`) and brings the AP up open. Old firmware (≤ 4.0.2)
  still parses so the app can offer the update; an old app cannot
  talk to firmware 4.0.3.
- **Docs:** new `docs/PROTOCOL-V2.2.md` (wire authority);
  `docs/KEY-LIFECYCLE-RUST.md` deleted; `THREAT-MODEL-RUST.md`
  rewritten (the WPA2 password is now the only boundary);
  `SECURITY-INPUT.md` and `ARCHITECTURE.md` updated; docs/ stays
  at exactly 6 files.
- **Rig:** pairing scenarios deleted; new connect-play scenarios
  (`connect-open-ap`, `connect-password-set`,
  `connect-password-reconnect`, `connect-key-reset-open`,
  `connect-usb`, `connect-migration-402-403`, `connect-unreachable`);
  HMAC benches deleted.

## [2.0.5] — 2026-11-20

Pairing confirm fix + full UI polish + cards layout + performance
(commit `b897188`, `a935db2`, `02bfced`):

- **Pairing fixed (research-first).** Root cause: the confirm block
  only showed while `pairing-active` was false, but every attempt
  sets it true until the 60 s window expires — confirm could never
  be clicked. Now the replace block shows whenever a replacement is
  required and the board is unpaired; "Replace pairing" is the
  primary action and reset actually deletes the stored key; the
  Overview banner offers confirm directly; the one-shot flag is
  board-bound; failure clears `pairing-active` and re-offers the
  block. Wi-Fi join banner + first-Unreachable notification.
- **UI polish.** Every popup closes on outside click; errors only as
  a centred ~480 px toast, a small inline box, or a banner — never
  full-window. UI logging added (`rsh-ui-YYYYMMDD.log` in
  `%APPDATA%\RSH\logs`).
- **Cards layout.** Gaps now auto-adjust to the window (spacers +
  min-width); cards are the same size ON and OFF (body stays in the
  tree, greyed when off).
- **Performance.** Release profile (strip + LTO fat): binaries
  −36.8% (daemon 36.65 → 22.05 MB, UI 42.33 → 27.90 MB). Frame rest
  (unchanged frames skip ~52 set calls; 10 Hz when not live);
  wakeup sources cut (USB-only port refresher; slow-poll 192.5 µs →
  9.3 µs); buffered UI log; tray refresh gating; window shown
  before the sync block, first paint logged; second job lane;
  backend lock fix; new perf gate step.

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
