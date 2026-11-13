# RSH v2.0.2 — Release notes (HOTFIX)

App v2.0.2 · Firmware v4.0.0 (unchanged) · Protocol v2.1

Hotfix on top of v2.0.1. Only the Windows app changes — **firmware
v4.0.0 is byte-identical to v2.0.1: if your board already runs firmware
v4.0.0, do NOT reflash it.**

## What changed in v2.0.2

### Blank window — the stale-process theory is ruled out

v2.0.1 was still blank on the first real Windows run: `RSH.exe`
started, then a completely blank window, no content, no tray icon,
no error, no crash — exactly the same as v2.0.0.

- **What the restart test proved:** the PC was restarted and a fresh
  v2.0.1 zip was run — still blank, still no tray. A restart kills
  every process, so a stale invisible v2.0.0 process holding the
  single-instance lock **cannot** have survived it. The stale-process
  theory (R-S1) is refuted: this is not an old process showing its old
  blank window. The v2.0.1 fix (independent `Loaded` steps, window
  shown before the runtime starts, stronger startup logging) is
  verified present in the shipped v2.0.1 binary, and it still did not
  paint — so the failure is further down, in the render path.
- **Focus of v2.0.2 — render failure:** the window is created and the
  app is running, but nothing paints. The prime suspects are now the
  FluentWindow / WPF-UI render path and GPU rendering on this PC
  (WPF-UI issue #1594 describes the same blank FluentWindow, and
  software rendering is the standard way to separate a GPU/driver
  render failure from an app bug). v2.0.2 therefore ships diagnostic
  flags that change only how the window renders — nothing else — so
  the next run on the real PC can tell us exactly where painting
  breaks. See "How to diagnose the blank window" below.

### Diagnostic flags (opt-in)

Two new command-line flags, for diagnosis only. Default behaviour is
unchanged — without a flag the app runs exactly as before.

- `--software-render` — forces software rendering (no GPU). If the
  window paints with this flag, the render failure is in the
  GPU/driver path.
- `--classic-window` — opens a plain WPF window instead of
  FluentWindow. If the window paints with this flag, the failure is
  in the FluentWindow / WPF-UI render path.

WPF-UI is on **4.3.0 (latest)** in this build.

### Startup hardening (also in this release)

These are valid product fixes in their own right — they are not the
cause of the blank window (see the restart test above), but they
remove traps that could hide or confuse a future diagnosis:

- **Version-aware single instance:** if an older-version `RSH.exe`
  is running, v2.0.2 terminates that process and starts itself,
  instead of handing off to a window that may never paint.
- **CloseToTray trap fixed:** if the tray icon was never created,
  closing the window now really exits the app instead of hiding it
  into a tray that does not exist.
- **Startup log records the version on its first line**, so the log
  itself proves which binary ran — no more guessing from a log that
  could belong to any build.

### How to diagnose the blank window (on the real PC, in this order)

1. Run `RSH.exe --software-render`. If the window paints, the GPU /
   driver render path is the problem — report that.
2. If still blank, run `RSH.exe --classic-window`. If the window
   paints, the FluentWindow / WPF-UI render path is the problem —
   report that.
3. If still blank, run the **plain-Window test build** — this
   release's separate download `RSH-render-test-x64.zip` (unzip it
   and run `RSH.RenderTest.exe`): a minimal build with a plain WPF
   window (no FluentWindow, no tray, no navigation), prepared by the
   debug workstream. If even that does not paint, the failure is
   below the app's UI code entirely (rendering / system level).
4. **Report the result** — which step first showed content, or that
   none did — together with `%AppData%\RSH\logs\rsh-<today>.log`.
   The log's first line now shows the app version, so it also
   confirms the binary that ran. Before reporting, check
   `RSH.exe` → Properties → Details → File version is **2.0.2**.

- **Verified:** `dotnet build` 0 errors; full Virtual Test Rig run
  (see verification below). The actual Windows paint check still has
  to happen on a real Windows PC — these flags only make that check
  conclusive.

## ⚠️ REQUIRED — .NET 10 Desktop Runtime (WPF)

Same as v2.0.1. **v2.0.2 does NOT bundle .NET. Without the .NET 10
Desktop Runtime the app will not start at all** — Windows will show
"You must install or update .NET to run this application" (or the app
will simply fail to launch).

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

## ⚠️ IMPORTANT — App is UNSIGNED in v2.0.2

Same as v2.0.1. **The Windows app in this release is NOT code-signed.**
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

Carried over from v2.0.1 — unchanged. The A7 measured run on real ESP32
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
  above. (v2.0.0 bundled the runtime; v2.0.1 and v2.0.2 do not.)
- ESP32 DevKit V1 board flashed with firmware v4.0.0 (see FLASH-GUIDE)
  — unchanged from v2.0.1, no reflash needed.
- vJoy driver (BrunnerInnovation 2.2.2.0) for game output.

## Known limitations (v2.0.2)

- App/MSIX unsigned (see above) — OV certificate comes later.
- A7 real-board run not done yet (see above) — board-side behaviour is
  verified in the Virtual Test Rig only.
- **.NET 10 Desktop Runtime is now a separate install** (see above) —
  without it the app will not start.
- Blank window on the real PC is still under diagnosis (see above) —
  v2.0.2 adds the flags and the test-build plan that make the next
  real-PC run conclusive; it does not yet claim the render failure
  is fixed.
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
  v2.0.2, **framework-dependent (loose)** (needs .NET 10 Desktop
  Runtime, see above).
- `firmware-signed.bin` (+ `SHA256SUMS.txt`) — signed OTA firmware image
  v4.0.0, unchanged from v2.0.1 (Ed25519, verified against the public key
  baked into the firmware).
- `RSH-Firmware-DevKitV1-v4.0.0.bin` (+ `SHA256SUMS.txt`) — signed,
  merged factory firmware image v4.0.0, unchanged from v2.0.1
  (bootloader + partitions + app).

The SBOM is in the repo at `docs/SBOM.cyclonedx.json` (not a release
asset). Raw unsigned binaries (`firmware.bin`, `bootloader.bin`,
`partitions.bin`) are deliberately NOT in this release — the merged
factory image contains all three byte-exact.
