# RSH — Support Period (v1.0.0)

**Version:** Rust app **v1.0.0** · C++ firmware **v4.0.1** · protocol **v2.1**
**Date:** 2026-11-14 · **Status:** documented policy (H5); the
coordinator may adjust the dates before the v1.0.0 release is tagged —
any change must be recorded here, not silently assumed.

## 1. Support window for v1.0.0

| Item | Window |
|---|---|
| **v1.0.0 supported from** | Its GitHub Release date (the `v1.0.0` tag date — fill in at release time; the release has not been tagged yet) |
| **v1.0.0 supported until** | **12 months after its release date, or 90 days after the next minor release (v1.1.0) ships — whichever is later** |
| **What "supported" means** | Bug fixes, critical security fixes, and compatibility fixes for Windows 10 22H2 / Windows 11 (the OS floor). New features go into the next minor, not into v1.0.x patches |
| **After the window ends** | v1.0.0 is end-of-life: no new fixes. Users upgrade to the current release (install over the old one; profiles and settings survive — see [USER-GUIDE.md](USER-GUIDE.md#updating)) |
| **Patch releases (v1.0.x)** | Released as needed inside the window for regressions and critical fixes only; each patch gets the same release process (signed firmware if firmware changes, SBOMs, `SHA256SUMS`) |

Example: if v1.0.0 releases 2026-11-14 and v1.1.0 ships 2027-03-01,
v1.0.0 is supported until 2027-11-14 (12 months after release — later
than 90 days after v1.1.0). If v1.1.0 ships 2027-10-01 instead, v1.0.0
is supported until 2027-12-30 (90 days after v1.1.0 — later than
2027-11-14).

## 2. What is covered

- **The app (v1.0.0 series):** crashes, blank/broken screens, pairing
  and connection failures, output-backend failures, profile loss or
  corruption, launcher and telemetry-switching defects.
- **The firmware (v4.0.1):** covered **as long as it ships with a
  supported app version**. Firmware v4.0.1 is unchanged from the C#
  series and is the only firmware version in scope; older firmware
  (< v4.0.0) is unsupported, v4.0.0 is supported only as a wire-format
  peer (see [FIRMWARE-COMPAT.md](FIRMWARE-COMPAT.md)).
- **Security:** critical security issues in the supported version are
  fixed inside the window regardless of the feature schedule. Report
  them with a diagnostics export (Settings → Diagnostics) and the log
  lines — never include keys or passwords.
- **Game compatibility:** the [GAME-MATRIX.md](GAME-MATRIX.md)
  verification (0 of 16 backend–game pairs verified in-game at
  v1.0.0) continues after release; a game becoming "verified" does not
  extend the v1.0.0 window, and an unverified game is not a support
  defect.

## 3. What is NOT covered

- **Windows versions below the floor** (older than Windows 10 22H2),
  ARM64 Windows, macOS / Linux as a runtime (the app is Windows-only;
  Linux builds exist only for the library crates in development).
- **vJoy itself** — vJoy is a third-party driver the user installs;
  RSH supports its own vJoy backend code, not the vJoy driver.
- **HIDMaestro itself** — a third-party driver/SDK (v2.1.0, MIT); RSH
  supports its sidecar and integration, not the HIDMaestro driver.
- **The C# series (v2.0.4 and earlier)** — superseded by the Rust
  recreation. C# users move to Rust v1.0.0 (re-pairing required — see
  [USER-GUIDE.md](USER-GUIDE.md#re-pairing-after-the-c-rust-switch-mandatory)).
  No C# patches are planned.
- **Hardware** — the ESP32 board, wiring, and physical wheel are the
  user's own build; firmware flashing help is in
  [FLASH-GUIDE.md](FLASH-GUIDE.md).

## 4. How to get help inside the window

1. Read [USER-GUIDE.md](USER-GUIDE.md#troubleshooting) — most issues
   (pairing window, firewall, vJoy missing, UAC prompts) are answered
   there.
2. Export diagnostics (Settings → Diagnostics) and note the log file
   (`%AppData%\RSH\logs\rsh-YYYYMMDD.log`).
3. Report via the GitHub repository issue tracker
   (`dreamsofts34-creator/RSH`) with the app version, firmware version,
   Windows build, backend in use (HIDMaestro / vJoy), and the
   diagnostics bundle. Never paste keys, Wi-Fi passwords, or keystore
   contents.

## 5. Known limitations accepted at v1.0.0 (not support defects)

These are documented, accepted limits of this release — reporting them
as bugs will get the same answer, so they are listed here plainly:

- **App ships unsigned** (OV certificate deferred) — SmartScreen
  warning is expected.
- **4-hour soak SKIPPED** by user decision 2026-11-14 14:54 (time
  constraint) — recorded as SKIPPED, never as passed. 60 s + 10 min
  soaks still run.
- **Game matrix 0/16 verified in-game** — verification is pending on
  Windows; unverified games are "not yet measured", not broken.
- **Updater is check-only** — the app cannot download/install updates
  in v1.0.0; updates are installed from GitHub Releases.
- **No force feedback** — the wheel has no FFB motor.
