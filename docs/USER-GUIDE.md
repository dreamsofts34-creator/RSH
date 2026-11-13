# RSH — User Guide

**Version 2.0.3 — for RSH app v2.0.3 / firmware v4.0.0 / protocol v2.1.**
(Previous guide version: 2.0.2 / firmware v4.0.0 — v2.0.3 adds
automatic classic window on Windows 10 (build < 22000, see §6) —
no flag needed — plus a `--fluent-window` escape hatch. Firmware
v4.0.0 is unchanged; if your board already runs v4.0.0, do not
reflash it.)

Wireless DIY sim-racing wheel: an ESP32 in the wheel sends its movement over
its own WiFi; RSH on the PC turns that into a standard game controller
(a vJoy virtual joystick) that every racing game understands.

**Principle:** the firmware stays dumb and fast (sample → send, 200 times a
second). Everything smart — calibration, smoothing, curves — happens here,
where you can change it without reflashing anything.

---

## 1. What you need

- The wheel, flashed with the RSH firmware v4.0.0 (ESP32 DevKit V1 build).
- Windows 11 (64-bit). Windows 10 (64-bit, build 19041+) also works —
  v2.0.3 automatically uses a classic window there (see §6). Windows 10
  is end-of-life (14 Oct 2025) — best-effort only, no guarantee.
- **.NET 10 Desktop Runtime 10.x (x64) — REQUIRED from v2.0.1.**
  The v2.0.3 app is framework-dependent: it does NOT bundle .NET
  anymore, so without this runtime `RSH.exe` will not start at all.
  Download it from the official Microsoft page
  <https://dotnet.microsoft.com/en-us/download/dotnet/10.0>
  (Run apps → **.NET Desktop Runtime 10.0.x**, Windows x64 installer),
  or run `winget install Microsoft.DotNet.DesktopRuntime.10`.
  The plain ".NET Runtime" alone is NOT enough (RSH is a WPF app),
  and .NET 8/9 does NOT count — it must be 10.x. Check with
  `dotnet --list-runtimes` — the output must contain a line
  `Microsoft.WindowsDesktop.App 10.x.x`.
- The **vJoy driver, BrunnerInnovation v2.2.2.0** — this is the signed
  release that works on Windows 11. Get the driver and `vJoyInterface.dll`
  from the same release; mixing versions does not work (RSH checks and
  tells you if they don't match).
- A game. Any game that accepts a standard joystick works.

## 2. Install the vJoy driver (once, as administrator)

1. Run the vJoy 2.2.2.0 installer (it needs admin — it's a driver).
2. Open **vJoyConf** (Configure vJoy).
3. Select device **1**. Enable axes **X**, **Y**, **Z** and **Slider**
   (Slider = handbrake, Phase G C8 — docs/HANDBRAKE-MAPPING.md). Set the number
   of buttons to at least 2 (32 gives you room for the future).
4. Click **Apply**. Leave "Enable vJoy" ticked.
5. Optional check: open **joy.cpl** (Set up USB game controllers) — you
   should see a vJoy device listed.

RSH itself finds `vJoyInterface.dll` automatically: it looks next to
the app first, then in the vJoy install folder
(`C:\Program Files\vJoy\x64`). No manual copying needed.

> Memory Integrity / HVCI note: if Windows blocks the driver, the Output
> page in RSH will say the driver is not enabled. This is a Windows
> policy on your machine, not an RSH fault.

## 3. Install RSH

**v2.0.3 app UNSIGNED hai** (user faisla 2026-11-13 — OV code-signing
certificate abhi nahi khareeda gaya, baad mein aayega). Is liye:

0. **Pehle .NET 10 Desktop Runtime install karo (v2.0.1 se REQUIRED).**
   `RSH.exe` chalane se pehle check karo: Command Prompt / PowerShell
   mein `dotnet --list-runtimes` chalao — output mein
   `Microsoft.WindowsDesktop.App 10.x.x` hona chahiye. Agar nahi hai
   (ya `dotnet` command hi nahi milti) to Microsoft se install karo:
   <https://dotnet.microsoft.com/en-us/download/dotnet/10.0>
   (Run apps → **.NET Desktop Runtime 10.0.x**, Windows x64 installer),
   ya `winget install Microsoft.DotNet.DesktopRuntime.10`. Sirf
   ".NET Runtime" kaafi nahi (WPF app hai), aur .NET 8/9 nahi chalega —
   10.x chahiye. v2.0.0 mein runtime zip ke andar bundled tha; v2.0.1
   mein nahi hai — yehi is hotfix ki packaging tabdeeli hai.
- **Portable build (recommended):** `RSH-windows-x64.zip` download karo,
  unzip karo, `RSH.exe` chalao — is ko signing ki zaroorat nahi, install
  bhi nahi hota. Yehi tareeqa v2.0.3 ke liye primary hai. Zip mein ab
  RSH.exe + us ki app DLLs hain (41 files) — framework-dependent (loose).
- **MSIX (`RSH-2.0.3.msix`):** unsigned package hai — Windows SmartScreen
  **"Windows protected your PC" / "Unknown publisher"** warning dikhayega
  (package par koi verified publisher nahi hai). Install karne ke liye
  Developer Mode (sideloading) chahiye ho sakta hai; pehla install test
  Windows PC par baaqi hai — us ke baad yahan exact steps likhe jayenge.
- **Download ki tasdeeq:** zip/MSIX chalane se pehle SHA-256 check karo —
  GitHub Release page par `SHA256SUMS-app.txt` se milao. Sirf official
  GitHub release (`github.com/dreamsofts34-creator/RSH/releases`) se
  download karo; kisi aur link se mila file mat chalao.
- **Firmware signed hai** (Ed25519) — app ka unsigned hona firmware
  signing se mutaliq nahi; board sirf signed firmware accept karta hai.

Your settings and profiles live in `%APPDATA%\RSH` and survive
updating — uninstall/reinstall never loses your calibration.

## 4. First run — the setup wizard

The wizard opens automatically the first time:

1. **vJoy check** — it turns green when the driver is feeding. If it stays
   red, the message on screen says exactly why (not installed, wrong
   version, device busy in another app, device not enabled in vJoyConf).
2. **Connect to the wheel** — power the wheel and join this PC's WiFi to
   the **RSH** network. The page goes green when packets arrive
   (you should see ~200 packets/sec).
   *While you race you are on the wheel's network, so this PC has no
   internet on that adapter. That is expected.*
   > **Pair immediately, at home.** Before the first pairing the `RSH`
   > network is **open (no password)** for 60 seconds so the app can
   > receive the wheel's per-device key and Wi-Fi password. During that
   > window anyone in Wi-Fi range could listen in — so pair in your own
   > room, right after flashing, and do not leave the board unpaired.
   > As soon as pairing succeeds the network becomes WPA2 with the
   > per-device password and the key is never sent again. If the window
   > expires before you pair, hold BOOT on the board for ≥ 10 s
   > (KEY_RESET) and pair again. Full detail: `docs/FLASH-GUIDE.md` step 6.
3. **Calibrate steering** — capture the centre, then the full range.
4. **Calibrate pedals** — capture released, then fully pressed.

You can re-run the wizard any time from **Settings → Run setup wizard**,
and fine-tune later on the Calibration page.

## 5. Everyday use

- Start RSH (or let autostart do it). It lives in the system tray;
  closing the window hides it there. Exit fully from the tray menu.
- The **Dashboard** shows the live wheel, pedals, link rate and whether
  the output is being fed. The toggle there pauses output instantly.
- **Safety watchdog:** if the wheel goes silent for 250 ms (out of range,
  battery, crash), the output snaps to neutral — centred steering, pedals
  off, no buttons — and recovers by itself when data returns. Your car
  never keeps "holding" a stuck input.
- **Profiles:** one per game. Calibration, processing and button mapping
  all live in the profile; switching is instant, even mid-session.
- **Updates:** Settings → About → **Check for updates**. RSH checks the
  GitHub Releases page for a newer stable version — it never downloads or
  installs anything by itself. Install updates only from the GitHub
  Releases page or the Microsoft Store, never from anywhere else.
  Firmware updates come through the setup wizard (Firmware step) or the
  board's update page — only signed official releases are accepted.
- **Processing defaults that work:** deadzone 2%, curve 0%, smoothing 30%.
  Set every in-game steering deadzone to **zero** and linearity to 100% —
  RSH does that job once, properly.

### In your game

1. Make sure joy.cpl shows the vJoy axes moving (steering = X,
   accelerator = Y, brake = Z, handbrake = Slider).
2. Bind: steering → vJoy X, throttle → vJoy Y, brake → vJoy Z,
   handbrake → vJoy Slider (separate axes), gear up/down → the vJoy
   buttons you chose on the Buttons page (defaults 1 and 2).
3. Set the in-game steering lock/range to your wheel's real physical
   range, deadzone 0, linearity 100%.
4. Old game that only offers one shared throttle/brake axis? Turn on
   **Processing → Combine pedals** for that game's profile.

## 6. Troubleshooting

| Symptom | Likely cause | Fix |
|---|---|---|
| `RSH.exe` won't start / "You must install or update .NET" | .NET 10 Desktop Runtime missing (v2.0.3 is framework-dependent) | Install .NET Desktop Runtime 10.x (x64) from <https://dotnet.microsoft.com/en-us/download/dotnet/10.0>, then check `dotnet --list-runtimes` shows `Microsoft.WindowsDesktop.App 10.x.x` |
| Blank window (no content, no tray icon) | FluentWindow chrome or navigation-area styling does not paint on Windows 10 (build < 22000) — v2.0.3 opens a classic window with a plain sidebar there automatically, no flag needed | On Windows 10 just run v2.0.3 normally. If it is still blank, try in this order and note which step first shows content: **(a)** `RSH.exe --software-render` — forces software rendering; **(b)** `RSH.exe --fluent-window` — forces the FluentWindow chrome on Windows 10, to test whether it paints after all; **(c)** the plain-Window test build (see `docs/RELEASE-NOTES-v2.0.3.md`) — a minimal window with no FluentWindow at all; **(d)** report the result (which step worked, or that none did) together with `%AppData%\RSH\logs\rsh-<today>.log`. On Windows 11 the extra diagnostic is `RSH.exe --plain-nav` (plain sidebar instead of the styled one). |
| Dashboard: "Wheel not connected" | PC not on RSH WiFi, wheel off, or HELO blocked | Join RSH, check wheel LED (solid = connected), check firewall allows RSH |
| Packets arrive but stop after 2 s | Windows dropped the WiFi (power saving) | In Device Manager, disable "allow the computer to turn off this device" for the WiFi adapter |
| Output red: "device BUSY" | Another feeder app holds the vJoy device | Close the other app (or pick a different device on the Output page) |
| Output red: "device missing" | Device not enabled in vJoyConf | vJoyConf → enable device 1 with X/Y/Z + Slider → Apply |
| Output red: DLL/driver version mismatch | vJoy pieces from different releases | Reinstall vJoy 2.2.2.0 completely |
| Steering drifts/jitters at centre | Sensor noise | Raise smoothing a little, or deadzone to 3–4% |
| Steering feels floaty | Too much smoothing | Lower smoothing toward 15–20% |
| Car pulls to one side | Centre moved (pot shifted) | Re-capture centre on the Calibration page |
| Pedals never reach 100% / always on | Range changed | Re-run pedal capture |
| High packet loss % on Diagnostics | WiFi congestion/interference | Move closer; firmware v4.0.0 auto-picks the quietest channel at boot — restart the wheel |

Something else? Open **Diagnostics → Export diagnostics (.zip)** and keep
the file — it contains the logs and the counters that explain what the
link was doing.

### Blank window — the app opens but shows nothing

**Windows 10 (build < 22000): v2.0.3 fixes this automatically.** The
blank window was the FluentWindow chrome (custom title bar) or the
navigation-area styling failing to paint on Windows 10 — a plain WPF
window on the same PC rendered fine (RenderTest: Render Tier 2,
hardware acceleration working), so the GPU and driver are ruled out.
The original v1.2.2 app used the same FluentWindow chrome and worked
on this PC, so the chrome alone is not proven to be the cause — that
is why v2.0.3 removes **both** suspects together on Windows 10: it
opens a **classic window** (plain OS title bar) **with a plain
sidebar** (literal text, system colours, default styling) by itself
— no flag needed, no setting to change. All 8 screens stay reachable
from the plain sidebar. On Windows 11 nothing changes: the app still
opens the normal FluentWindow with the styled sidebar.

If `RSH.exe` starts (you may see it in Task Manager) but the window is
still completely blank and no tray icon appears, the window is most
likely failing to *render* — the app itself is running. Try these in
order, one at a time:

1. **Software rendering:** close RSH completely, then start it from
   a Command Prompt with
   `RSH.exe --software-render`
   This forces WPF to draw without the GPU. If the window appears now,
   the GPU driver (or its render thread) is the problem — update the
   GPU driver from the manufacturer's site and try again without the
   flag. The log line `Software rendering requested (Render Tier: X)`
   in `%APPDATA%\RSH\logs` confirms the flag was active.
2. **Classic window chrome:** close RSH, then start it with
   `RSH.exe --classic-window`
   This keeps the same app but uses a plain Windows title bar instead
   of the custom FluentWindow chrome. On Windows 10 this is already
   the default in v2.0.3 — the flag forces it on Windows 11 too, to
   test whether the FluentWindow chrome is what fails there.
3. **Plain sidebar:** close RSH, then start it with
   `RSH.exe --plain-nav`
   This keeps the FluentWindow chrome but replaces the styled sidebar
   with a plain one (literal text, system colours, default styling —
   like the original v1.2.2 sidebar). On Windows 10 this is already
   the default in v2.0.3 — the flag forces it on Windows 11 too, to
   test whether the sidebar styling is what fails there.
4. **FluentWindow escape hatch:** close RSH, then start it with
   `RSH.exe --fluent-window`
   This forces the FluentWindow chrome even on Windows 10 — use it
   only to test whether FluentWindow paints after all (for example
   after a Windows update). If both `--classic-window` and
   `--fluent-window` are given, classic wins. `--fluent-window` also
   turns the plain sidebar off (full modern UI).
5. **All together:** `RSH.exe --software-render --classic-window --plain-nav`
   narrows it further — if even this stays blank, the problem is below
   WPF itself (GPU/OS level).
6. **Disable hardware acceleration for all WPF apps** (manual test):
   in Registry Editor create DWORD
   `HKEY_CURRENT_USER\SOFTWARE\Microsoft\Avalon.Graphics\DisableHWAcceleration`
   = `1`, restart RSH, and remove the value afterwards if it made no
   difference.
7. **Render test:** the release ships a tiny separate tool,
   `RSH.RenderTest.exe` — a plain window with one button and no
   WPF-UI at all (it also accepts `--software-render`). If *that*
   window renders but RSH stays blank, the WPF-UI window chrome is
   the culprit; if it is blank too, the problem is the GPU/OS, not RSH.

No flag changes any setting — they only affect that one launch.
If a flag fixes it, tell us which one (Diagnostics → Export
diagnostics) so the fix can be made permanent.

> **Stale-process note:** agar update ke baad bhi purani blank window
> dikhe to Task Manager mein check karo ke koi purani `RSH.exe` to nahi
> chal rahi — v2.0.2 ka version-aware single instance purane-version
> process ko khud terminate karke start hota hai, is liye aam tor par ye
> karne ki zaroorat nahi parni chahiye.

## 7. Performance (by design)

RSH is built to disappear on low-end PCs:

- The 200 Hz pipeline allocates **zero bytes per tick** on its hot path
  (measured: ~0.45 µs of work per tick).
- Idle: ~1 keepalive packet per second, UI asleep, engine waiting.
- The interface redraws at 30 Hz and only for the page you are looking
  at; graphs pause when you leave the page or minimise.
- Budgets it is held to: ≤1% CPU idle, ≤3% while racing, ≤120–150 MB
  working set. The bundled `RSH.Benchmarks` tool prints the real
  numbers on your machine.

## 8. Files and folders

| What | Where |
|---|---|
| App | `%LOCALAPPDATA%\Programs\RSH` |
| Settings | `%APPDATA%\RSH\settings.json` |
| Profiles | `%APPDATA%\RSH\profiles.json` (a `.bak` is kept automatically) |
| Logs | `%APPDATA%\RSH\logs` (daily, 7 days kept) |

Settings and profiles are saved atomically with a backup — a crash or
power cut mid-save cannot corrupt them; on any read failure RSH
falls back to the backup, then to sane defaults. It never crashes because
of a broken config file.
