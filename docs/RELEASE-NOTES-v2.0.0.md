# RSH v2.0.0 — Release notes

App v2.0.0 · Firmware v4.0.0 · Protocol v2.1

## ⚠️ IMPORTANT — App is UNSIGNED in v2.0.0

**The Windows app in this release is NOT code-signed.**
The OV code-signing certificate has not been bought yet — it will come later.
What this means for you:

- Windows SmartScreen will warn **"Windows protected your PC" / "Unknown publisher"** when you run the app. This is expected — the app is unsigned, not malicious.
- **Verify your download first:** check the SHA-256 of the file against
  `SHA256SUMS-app.txt` (app) / `SHA256SUMS.txt` (firmware) in this release,
  and download only from the official GitHub release page
  (`github.com/dreamsofts34-creator/RSH/releases`). Never run a copy from
  any other link.
- **Recommended install:** the portable build `RSH-windows-x64.zip` —
  unzip and run `RSH.exe` (no signing needed, no install needed).

**Firmware IS signed** (Ed25519) — this is separate from app signing.
The board only accepts signed firmware; the app being unsigned does not
affect firmware security.

A signed app/MSIX will come in a later release, after the OV certificate
is bought (see `docs/SIGNING.md`). MSIX packaging is prepared in
`software/packaging/msix/` but is not part of this release.

## ⚠️ Real-board run (A7) — NOT done yet

The A7 measured run on real ESP32 hardware has **not** been performed for
this release. All testing so far was done in the Virtual Test Rig
(virtual wheel + real app/firmware code, no real board). The A7 runbook is
`docs/A7-RUNBOOK.md` — the board run is done once, on real hardware, by
the user: flash the board, run the app, measure sent/s ≈ 200, overruns 0,
maxCycleUs < 3500 µs. Until that run passes, board-side numbers (200 Hz
stream on hardware, flash-encryption / Secure Boot board steps) are
unverified on real hardware.

## What's in v2.0.0

- Complete rebuild of the RSH wheel system (firmware v4.0.0 + app v2.0.0):
  per-module tuning (Steering, Accel, Brake, Clutch, Handbrake, Shifter,
  Horn, Buttons, Encoders, LED), GPIO pin setup in Settings, game launcher
  on the Dashboard, profiles, monitor + performance panel, light + dark
  themes.
- Protocol v2.1 (signed control channel, per-device key, HMAC) and signed
  firmware with anti-downgrade protection.
- Wired USB-serial transport alongside Wi-Fi; per-car telemetry readers
  (Assetto Corsa, ACC, iRacing); LED strip (WS2812) support.
- vJoy output (BrunnerInnovation 2.2.2.0). HIDMaestro backend stays
  experimental — vJoy is the default for v2.0.0.

## Requirements

- Windows 11 (x64), .NET 10 (bundled in the portable build).
- ESP32 DevKit V1 board flashed with firmware v4.0.0 (see FLASH-GUIDE).
- vJoy driver (BrunnerInnovation 2.2.2.0) for game output.

## Known limitations (v2.0.0)

- App/MSIX unsigned (see above) — OV certificate comes later.
- A7 real-board run not done yet (see above) — board-side behaviour is
  verified in the Virtual Test Rig only.
- Firewall rule is created on first run and is **not** removed on
  uninstall — run `software/packaging/firewall/uninstall-firewall-rule.ps1`
  (or remove it from Windows Firewall settings).
- Flash encryption + Secure Boot procedure is documented
  (`docs/FLASH-ENCRYPTION-PROCEDURE.md`) but needs a board-side run;
  NVS password is obfuscated, not encrypted, until that is done.
- 4-hour long soak was stopped early by user order (~16 min in, no metrics
  collected) — soak evidence is the 60 s soak in the virtual test rig.

## Files in this release

- `RSH-windows-x64.zip` (+ `SHA256SUMS-app.txt`) — portable Windows app.
- `firmware-signed.bin` (+ `SHA256SUMS.txt`) — signed OTA firmware image
  (Ed25519, verified against the public key baked into the firmware).
- `RSH-Firmware-DevKitV1-v4.0.0.bin` (+ `SHA256SUMS.txt`) — signed,
  merged factory firmware image (bootloader + partitions + app).

The SBOM is in the repo at `docs/SBOM.cyclonedx.json` (not a release
asset). Raw unsigned binaries (`firmware.bin`, `bootloader.bin`,
`partitions.bin`) are deliberately NOT in this release — the merged
factory image contains all three byte-exact.
