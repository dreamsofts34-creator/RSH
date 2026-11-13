# RSH v2.0.3 — Release notes (HOTFIX)

App v2.0.3 · Firmware v4.0.0 (unchanged) · Protocol v2.1

Hotfix on top of v2.0.2. Only the Windows app changes — **firmware
v4.0.0 is byte-identical to v2.0.2: if your board already runs firmware
v4.0.0, do NOT reflash it.**

## What changed in v2.0.3

### Blank window — fixed on Windows 10 (automatic classic window + plain sidebar)

v2.0.2 was still blank on the first real Windows run: `RSH.exe`
started, then a completely blank window, no content, no tray icon,
no error, no crash — exactly the same as v2.0.0 and v2.0.1.

- **What the RenderTest proved:** the plain-Window test build
  (`RSH-render-test-x64.zip`, shipped with v2.0.2) **ran fine on the
  same PC** — a plain WPF window rendered and responded (button
  clicked), with Render Tier 131072 (0x20000 = Tier 2, hardware
  acceleration). The GPU and driver are therefore ruled out: WPF
  itself paints on this PC.
- **What the PC turned out to be:** the PC runs **Windows 10,
  build 10.0.19045.0 (22H2)** — not Windows 11. The plan's floor
  was Windows 11, so this was never tested.
- **What the original build proved:** the original v1.2.2 app used
  the **same** FluentWindow chrome (`WindowBackdropType="None"`,
  `ExtendsContentIntoTitleBar="True"`, WPF-UI 4.3.0) and **worked**
  on this same Windows 10 PC. So the chrome alone is not proven to
  be the cause. The real differences from the original are the
  navigation-area styling (UiStrings bindings, DynamicResource theme
  brushes, `RSHNavStyle`) and the new pages — either the chrome or
  the navigation area (or both) can be what fails to paint.
- **The fix in v2.0.3:** on Windows 10 (build < 22000) the app now
  removes **both** suspects together, **automatically** — it opens a
  **classic window** (plain OS title bar) **with a plain sidebar**
  (literal text, system colours, default styling, like the original
  v1.2.2 sidebar). No flag needed, no setting to change, all 8
  screens stay reachable from the plain sidebar. On Windows 11
  nothing changes: the app still opens the normal FluentWindow with
  the styled sidebar, exactly as before. The setup wizard uses the
  same rule. This is a product fix, not a diagnostic: a Windows 10
  user just runs `RSH.exe` and gets a window that paints.

### Command-line flags (overrides only)

The v2.0.2 diagnostic flags are still there, and one new escape
hatch is added. Default behaviour is now: Windows 10 → classic,
Windows 11 → FluentWindow. The flags only override that default
for one launch — they change no setting.

- `--software-render` — forces software rendering (no GPU). If the
  window paints with this flag, the render failure is in the
  GPU/driver path.
- `--classic-window` — forces a plain WPF window instead of
  FluentWindow. On Windows 10 this is already the default in
  v2.0.3; the flag forces it on Windows 11 too.
- `--plain-nav` — forces a plain sidebar (literal text, system
  colours, default styling) instead of the styled one. On Windows 10
  this is already the default in v2.0.3; the flag forces it on
  Windows 11 too.
- `--fluent-window` — **new escape hatch:** forces the FluentWindow
  chrome even on Windows 10. Use it only to test whether
  FluentWindow paints after all (for example after a Windows
  update). If both `--classic-window` and `--fluent-window` are
  given, classic wins. `--fluent-window` also turns the plain sidebar
  off (full modern UI).

WPF-UI is on **4.3.0 (latest)** in this build, as in v2.0.2.

### Carried over from v2.0.2 (unchanged)

- **Version-aware single instance:** if an older-version `RSH.exe`
  is running, v2.0.3 terminates that process and starts itself,
  instead of handing off to a window that may never paint.
- **CloseToTray trap fixed:** if the tray icon was never created,
  closing the window now really exits the app instead of hiding it
  into a tray that does not exist.
- **Startup log records the version on its first line**, so the log
  itself proves which binary ran — no more guessing from a log that
  could belong to any build.
- **Autostart stale-entry repair:** a Run key pointing at an old
  install folder is repaired at startup instead of launching the
  old blank build at login.

### How to verify on the real PC (in this order)

1. Run `RSH.exe` normally (no flag) on Windows 10. The window
   should now open with a plain title bar and a plain sidebar and
   show content — no flag needed. If it does, the fix works — report
   that, together with `%AppData%\RSH\logs\rsh-<today>.log`.
2. If it is still blank, run `RSH.exe --software-render`. If the
   window paints, the GPU / driver render path is the problem —
   report that.
3. If still blank, run `RSH.exe --fluent-window`. If the window
   paints, FluentWindow works on this PC after all and the
   automatic classic choice was wrong for it — report that.
4. If still blank, run `RSH.exe --plain-nav` on Windows 11 (on
   Windows 10 this is already the default). If the window paints,
   the sidebar styling was the problem — report that.
5. If still blank, run the **plain-Window test build** — this
   release's separate download `RSH-render-test-x64.zip` (unzip it
   and run `RSH.RenderTest.exe`): a minimal build with a plain WPF
   window (no FluentWindow, no tray, no navigation). It rendered
   on this PC in v2.0.2 testing; if it does not render now, the
   failure is below the app's UI code entirely (rendering /
   system level).
6. **Report the result** — which step first showed content, or that
   none did — together with `%AppData%\RSH\logs\rsh-<today>.log`.
   The log's first line shows the app version, so it also
   confirms the binary that ran. Before reporting, check
   `RSH.exe` → Properties → Details → File version is **2.0.3**.

- **Verified:** `dotnet build` 0 errors; full Virtual Test Rig run
  (see verification below). The actual Windows paint check still has
  to happen on a real Windows PC — the automatic classic window is
  code, and only the real-PC run proves it paints there.

## ⚠️ REQUIRED — .NET 10 Desktop Runtime (WPF)

Same as v2.0.1 and v2.0.2. **v2.0.3 does NOT bundle .NET. Without the
.NET 10 Desktop Runtime the app will not start at all** — Windows
will show "You must install or update .NET to run this application"
(or the app will simply fail to launch).

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

## ⚠️ IMPORTANT — App is UNSIGNED in v2.0.3

Same as v2.0.1 and v2.0.2. **The Windows app in this release is NOT
code-signed.** The OV code-signing certificate has not been bought
yet — it will come later. What this means for you:

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

Carried over from v2.0.1 and v2.0.2 — unchanged. The A7 measured run
on real ESP32 hardware has **not** been performed for this release.
All testing so far was done in the Virtual Test Rig (virtual wheel +
real app/firmware code, no real board). The A7 runbook is
`docs/A7-RUNBOOK.md` — the board run is done once, on real hardware,
by the user: flash the board, run the app, measure sent/s ≈ 200,
overruns 0, maxCycleUs < 3500 µs. Until that run passes, board-side
numbers (200 Hz stream on hardware, flash-encryption / Secure Boot
board steps) are unverified on real hardware.

## Requirements

- Windows 10 (64-bit, build 19041+) or Windows 11 (x64). On
  Windows 10 the app opens a classic window automatically (see
  above); on Windows 11 it opens the normal FluentWindow.
- **.NET 10 Desktop Runtime 10.x (x64)** — install separately, see
  above. (v2.0.0 bundled the runtime; v2.0.1, v2.0.2 and v2.0.3 do
  not.)
- ESP32 DevKit V1 board flashed with firmware v4.0.0 (see FLASH-GUIDE)
  — unchanged from v2.0.2, no reflash needed.
- vJoy driver (BrunnerInnovation 2.2.2.0) for game output.

## Known limitations (v2.0.3)

- App/MSIX unsigned (see above) — OV certificate comes later.
- A7 real-board run not done yet (see above) — board-side behaviour is
  verified in the Virtual Test Rig only.
- **.NET 10 Desktop Runtime is now a separate install** (see above) —
  without it the app will not start.
- Blank window on the real PC: v2.0.3 changes the window
  automatically to classic + plain sidebar on Windows 10, which
  removes both suspects at once — but the real-PC paint check has
  not happened yet for this build, so the fix is not yet confirmed
  on the user's PC (see "How to verify" above).
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
  v2.0.3, **framework-dependent (loose)** (needs .NET 10 Desktop
  Runtime, see above).
- `RSH-render-test-x64.zip` (+ `SHA256SUMS-app.txt`) — plain-Window
  diagnostic build (`RSH.RenderTest.exe`, zero NuGet dependencies),
  unchanged from v2.0.2.
- `firmware-signed.bin` (+ `SHA256SUMS.txt`) — signed OTA firmware image
  v4.0.0, unchanged from v2.0.2 (Ed25519, verified against the public key
  baked into the firmware).
- `RSH-Firmware-DevKitV1-v4.0.0.bin` (+ `SHA256SUMS.txt`) — signed,
  merged factory firmware image v4.0.0, unchanged from v2.0.2
  (bootloader + partitions + app).

The SBOM is in the repo at `docs/SBOM.cyclonedx.json` (not a release
asset). Raw unsigned binaries (`firmware.bin`, `bootloader.bin`,
`partitions.bin`) are deliberately NOT in this release — the merged
factory image contains all three byte-exact.
