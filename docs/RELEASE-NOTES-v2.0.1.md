# RSH v2.0.1 — Release notes (HOTFIX)

App v2.0.1 · Firmware v4.0.0 (unchanged) · Protocol v2.1

Hotfix on top of v2.0.0. Only the Windows app packaging and the
blank-window bug change — **firmware v4.0.0 is byte-identical to v2.0.0:
if your board already runs firmware v4.0.0, do NOT reflash it.**

## What changed in v2.0.1

### Blank-window fix

What was seen on the first real Windows run of v2.0.0: `RSH.exe` started
and then went silent — a completely blank window, no content, no tray
icon, no error, no crash. The app was stuck at startup.

- **Root cause (most likely):** `MainWindow`'s `Loaded` handler was one
  all-or-nothing chain (apply theme → set up tray → start UI timer →
  navigate to Dashboard). If any single step threw — most likely the
  tray setup (`NotifyIcon.Register()`, a native shell call) — the rest
  never ran: no tray, no timer, no first navigation, so the window stayed
  blank. The exception was swallowed by the global handler (app kept
  running), which is exactly what was seen. A second trap: with
  minimized start the window was never shown at all, so `Loaded` never
  fired and no tray icon was ever created.
- **Fix:** the four `Loaded` steps are now independent, each in its own
  `try/catch` with logging — one step failing can no longer kill the
  others; a failed page navigation shows a placeholder with the reason
  instead of blank content; a failed tray registration only logs a
  warning. The window is now shown **before** the runtime starts
  (runtime starts on a background thread), so the first paint never waits
  for services. Startup logging is much stronger: the log now records
  every milestone (`RSH started` → services → theme → window created →
  window shown → runtime started → tray icon → first navigation).
- **Verified:** `dotnet build` 0 errors; freeze/hang audit of the UI
  thread (no `.Result()`/`.Wait()`/`Thread.Sleep` in `RSH.App`, U-P1–U-P10
  checked); full Virtual Test Rig run (see verification below). The actual
  Windows paint/tray check still has to happen on a real Windows PC —
  this is the first build with the fix.
- **If it happens again:** open `%AppData%\RSH\logs\rsh-<today>.log` —
  it now shows exactly which step failed (`Tray icon setup failed` /
  `Failed to create page` / `Theme apply failed`).

### New packaging: framework-dependent (loose)

- The v2.0.1 zip is a **framework-dependent (loose)** build:
  the zip contains `RSH.exe` plus its app DLLs next to it (41 files),
  clean and organised — no more 150+ flat files, no bundled .NET.
- Because the runtime is no longer bundled, the download is much
  smaller than v2.0.0's self-contained zip.
- **Trade-off (read the next section): the .NET 10 Desktop Runtime
  must now be installed on the PC separately.**

## ⚠️ REQUIRED — .NET 10 Desktop Runtime (WPF)

**v2.0.1 does NOT bundle .NET anymore. Without the .NET 10 Desktop
Runtime the app will not start at all** — Windows will show
"You must install or update .NET to run this application" (or the app
will simply fail to launch). This is the one new thing v2.0.1 asks of
you, and it takes about a minute.

- **Download (official Microsoft page):**
  <https://dotnet.microsoft.com/en-us/download/dotnet/10.0>
  — on that page, under **Run apps – Runtime**, pick
  **.NET Desktop Runtime 10.0.x, Windows x64 installer**
  (file name `windowsdesktop-runtime-10.0.x-win-x64.exe`).
  Only the **Desktop** Runtime works — the plain ".NET Runtime" alone
  is NOT enough, because RSH is a WPF desktop app.
- **.NET 8 or .NET 9 does NOT count.** The app targets .NET 10, so an
  older runtime will not satisfy it. Versions install side by side,
  so installing 10.x will not disturb anything else on your PC.
- **Alternative (command line):**
  `winget install Microsoft.DotNet.DesktopRuntime.10`
- **Check whether you already have it** (Command Prompt / PowerShell):
  `dotnet --list-runtimes`
  Look for a line `Microsoft.WindowsDesktop.App 10.x.x` in the output.
  If that line is there, you are done — just run `RSH.exe`.
  If the command itself is not recognised (`dotnet` not found), the
  runtime is not installed — install it from the link above first.

## ⚠️ IMPORTANT — App is UNSIGNED in v2.0.1

Same as v2.0.0. **The Windows app in this release is NOT code-signed.**
The OV code-signing certificate has not been bought yet — it will come
later. What this means for you:

- Windows SmartScreen will warn **"Windows protected your PC" /
  "Unknown publisher"** when you run the app. This is expected — the
  app is unsigned, not malicious.
- **Verify your download first:** check the SHA-256 of the file against
  `SHA256SUMS-app.txt` (app) / `SHA256SUMS.txt` (firmware) in this
  release, and download only from the official GitHub release page
  (`github.com/dreamsofts34-creator/RSH/releases`). Never run a copy
  from any other link.
- **Recommended install:** the portable build `RSH-windows-x64.zip` —
  unzip and run `RSH.exe` (no signing needed, no install needed).

**Firmware IS signed** (Ed25519) — this is separate from app signing.
The board only accepts signed firmware; the app being unsigned does not
affect firmware security.

A signed app/MSIX will come in a later release, after the OV certificate
is bought (see `docs/SIGNING.md`). MSIX packaging is prepared in
`software/packaging/msix/` but is not part of this release.

## ⚠️ Real-board run (A7) — NOT done yet

Carried over from v2.0.0 — unchanged. The A7 measured run on real ESP32
hardware has **not** been performed for this release. All testing so far
was done in the Virtual Test Rig (virtual wheel + real app/firmware
code, no real board). The A7 runbook is `docs/A7-RUNBOOK.md` — the
board run is done once, on real hardware, by the user: flash the board,
run the app, measure sent/s ≈ 200, overruns 0, maxCycleUs < 3500 µs.
Until that run passes, board-side numbers (200 Hz stream on hardware,
flash-encryption / Secure Boot board steps) are unverified on real
hardware.

## Requirements

- Windows 11 (x64).
- **.NET 10 Desktop Runtime 10.x (x64)** — install separately, see
  above. (v2.0.0 bundled the runtime; v2.0.1 does not.)
- ESP32 DevKit V1 board flashed with firmware v4.0.0 (see FLASH-GUIDE)
  — unchanged from v2.0.0, no reflash needed.
- vJoy driver (BrunnerInnovation 2.2.2.0) for game output.

## Known limitations (v2.0.1)

- App/MSIX unsigned (see above) — OV certificate comes later.
- A7 real-board run not done yet (see above) — board-side behaviour is
  verified in the Virtual Test Rig only.
- **.NET 10 Desktop Runtime is now a separate install** (see above) —
  without it the app will not start.
- Firewall rule is created on first run and is **not** removed on
  uninstall — run `software/packaging/firewall/uninstall-firewall-rule.ps1`
  (or remove it from Windows Firewall settings).
- Flash encryption + Secure Boot procedure is documented
  (`docs/FLASH-ENCRYPTION-PROCEDURE.md`) but needs a board-side run;
  NVS password is obfuscated, not encrypted, until that is done.
- 4-hour long soak was stopped early by user order (~16 min in, no metrics
  collected) — soak evidence is the 60 s soak in the virtual test rig.

## Files in this release

- `RSH-windows-x64.zip` (+ `SHA256SUMS-app.txt`) — portable Windows app
  v2.0.1, **framework-dependent (loose)** (needs .NET 10 Desktop
  Runtime, see above).
- `firmware-signed.bin` (+ `SHA256SUMS.txt`) — signed OTA firmware image
  v4.0.0, unchanged from v2.0.0 (Ed25519, verified against the public key
  baked into the firmware).
- `RSH-Firmware-DevKitV1-v4.0.0.bin` (+ `SHA256SUMS.txt`) — signed,
  merged factory firmware image v4.0.0, unchanged from v2.0.0
  (bootloader + partitions + app).

The SBOM is in the repo at `docs/SBOM.cyclonedx.json` (not a release
asset). Raw unsigned binaries (`firmware.bin`, `bootloader.bin`,
`partitions.bin`) are deliberately NOT in this release — the merged
factory image contains all three byte-exact.
