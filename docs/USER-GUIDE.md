# RSH — User Guide (Rust app v1.0.1)

**App:** RSH v1.0.1 (Tauri 2, Windows 10 22H2 / Windows 11, x64) ·
**Wheel firmware:** C++ v4.0.1 · **Protocol:** v2.1 ·
**Board:** ESP32 DevKit V1

This guide is for the **Rust recreation** of RSH. If you used the old
C# app (v2.0.4 or earlier), read
[Re-pairing after the C# → Rust switch](#re-pairing-after-the-c-rust-switch-mandatory)
first — your old pairing does **not** carry over.

## Contents

- [Install](#install)
- [Unsigned app — SmartScreen](#unsigned-app--smartscreen)
- [First run — the Setup wizard](#first-run--the-setup-wizard)
- [Pairing](#pairing)
- [Re-pairing after the C# → Rust switch (mandatory)](#re-pairing-after-the-c-rust-switch-mandatory)
- [Connecting — Wi-Fi or USB](#connecting--wi-fi-or-usb)
- [The 8 screens](#the-8-screens)
- [Per-module tuning](#per-module-tuning)
- [Profiles](#profiles)
- [Game launcher](#game-launcher)
- [Output backend — HIDMaestro or vJoy](#output-backend--hidmaestro-or-vjoy)
- [Updating](#updating)
- [Troubleshooting](#troubleshooting)
- [Uninstall](#uninstall)

## Install

1. Download **`RSH_1.0.1_x64-setup.exe`** (primary installer) or
   **`RSH_1.0.1_x64_en-US.msi`** (secondary, for managed installs)
   from the
   [v1.0.1 GitHub Release](https://github.com/dreamsofts34-creator/RSH/releases/tag/v1.0.1).
2. **Verify the download** before running it — compare the SHA-256
   with `SHA256SUMS` in the same release:
   - PowerShell: `Get-FileHash RSH_1.0.1_x64-setup.exe -Algorithm SHA256`
   - If the hash does not match, delete the file and download it again.
     Never run a mismatched installer.
3. Double-click the installer. It installs for the current user only —
   **no admin prompt at install time**. It creates a Start Menu
   shortcut and an entry in "Apps & features".
4. **No .NET runtime is needed.** The app is a native Tauri binary.
   On a machine without WebView2, the installer downloads it silently
   (internet required at install time).
5. **Firmware:** if your wheel already runs firmware **v4.0.1**, you do
   **not** need to reflash anything — the Rust app talks to the same
   board. Only a new/blank board or a board that will not boot needs
   flashing: see [FLASH-GUIDE.md](FLASH-GUIDE.md).

### Unsigned app — SmartScreen

v1.0.1 ships **unsigned** (the OV code-signing certificate is deferred
to a later release). Windows SmartScreen will warn when you run the
installer — this is expected. Click "More info" → "Run anyway".
Firmware signing is unaffected: the firmware image is still
Ed25519-signed and verified by the board.

## First run — the Setup wizard

The first launch opens the **Setup wizard**:

1. **Connect** — find your wheel (Wi-Fi or USB-serial).
2. **Pair** — exchange the per-device key (pair button or USB, below).
3. **Wi-Fi password** — the password for the wheel's own `RSH`
   Wi-Fi network. The wheel is a Wi-Fi access point only — it never
   joins your home network, and this password is for its network,
   not your home router's.
4. **Pins** — assign GPIO pins to steering, pedals, buttons, LED, etc.
   (validated against the board's pin rules before anything is sent).
5. **Calibrate** — steering lock-to-lock, pedal ranges.
6. **Firmware** — the wizard shows the board's firmware version and
   refuses a downgrade upload.
7. **Finish** — you land on the Dashboard.

You can re-run Setup any time from Settings. Skipping calibration is
possible, but steering/pedals will read uncalibrated until you do it.

## Pairing

Pairing is how the app and the wheel agree on a secret key. The board
generates one random 32-byte key per board, and discloses it **exactly
once, inside a pairing window**. The app stores the key in **Windows
Credential Manager** (service `RSH`) — never in a file, never in a log.
Once paired, the app and the wheel authenticate every control message
with HMAC (protocol v2.1).

**The wheel is a Wi-Fi access point only.** It creates its own `RSH`
Wi-Fi network, and your PC joins that network to talk to the wheel.
The wheel **never joins your home network** — there is no home-network
setup step, and no home-network password is ever asked for or stored.
The only Wi-Fi password in RSH is the password of the wheel's own
`RSH` network.

### Pairing over USB (recommended)

Plug the wheel in with a **data** USB cable (charge-only cables do not
work). If the board is unpaired, the app opens a pairing window
**on demand** as soon as it sees the board over serial — there is no
countdown to race against, because physical access to the board is the
trust anchor. Pair from the app and you are done.

### Pairing over Wi-Fi — the pair button (firmware v4.0.2)

On firmware v4.0.2, an **unpaired** board has a pair button: hold the
board's **BOOT** button for **3 seconds** (and release before 5
seconds — 5 seconds is safe mode, see below). The board's LED blinks
(3 Hz) while the pairing window is open, and the window stays open
for **120 seconds**. Pair from the app inside that window.

- The **3-second hold works only on an unpaired board.** On a board
  that is already paired, holding BOOT for 3 seconds does nothing —
  that is by design, so nobody can re-pair a wheel that is not theirs.
  To re-pair an already-paired board, reset its pairing first (hold
  BOOT for **10 seconds or more** — KEY_RESET, below) and then pair
  it as a fresh board.
- The app shows the **real remaining time** of the window. Never start
  pairing after the countdown reaches zero — the window is closed and
  the key cannot be read out of the board any more.
- A board that has just booted also opens a **60-second** pairing
  window on its own, from the moment its `RSH` network comes up. The
  pair button is the way to open a fresh window later, without
  rebooting the wheel.

### The Wi-Fi password — shown once

When pairing succeeds, the app shows the password of the wheel's
`RSH` network **once**. Save it somewhere safe before you confirm —
the app drops it after you confirm you saved it, and the board never
shows it again. (If you lose it, the password lives in the Windows
Credential Manager entry for the wheel, and the app can show it again
from there after a confirmation dialog.)

After pairing, the wheel locks its network with WPA2 using that
password, and the app **rejoins the `RSH` network itself** (it creates
a Windows Wi-Fi profile for `RSH` and connects to it), so you do not
have to switch networks by hand. If the automatic rejoin does not
work, the app shows a card with the exact steps to join `RSH`
manually — the password you just saved is the one to use.

### If the window closes before pairing finishes

Pairing is over for that attempt: open a fresh window — hold BOOT for
3 seconds (unpaired board, firmware v4.0.2), plug in USB, or reboot
the wheel for a fresh 60-second boot window — and try again. There is
no way to read the key out of the board after the window closes —
that is by design.

### Re-pairing after the C# → Rust switch (mandatory)

If you are moving from the C# app (v2.0.4 or earlier) to the Rust app,
**you must re-pair your wheel**. The C# app stored its key as a DPAPI
blob in `%AppData%\RSH`; the Rust app stores its key as a `keyring`
entry in Windows Credential Manager. These are different stores and
the Rust app **cannot read the C# key** — there is deliberately no
"import the old key" path (that path would itself be a key-leak
vector).

Steps:

1. **KEY_RESET on the board:** hold the **BOOT** button for **10
   seconds or more** (hold ≥ 10 s arms key-reset; 3 s is the pair
   button on firmware v4.0.2, 5–10 s is safe mode — see
   [FLASH-GUIDE.md](FLASH-GUIDE.md#4-boot-hold-safe-mode-key-reset-no-reflash-needed)).
   The board generates a fresh key and a fresh Wi-Fi password.
2. Pair again from the app — USB, or the 3-second pair-button hold
   (firmware v4.0.2), or within the 60-second boot window.
3. Your old C# key stops working the moment you move to the Rust app.

**Profiles and settings are separate from keys.** The Rust app starts
fresh and never auto-migrates C# data; a read-only importer for C#
profiles exists for manual use only. Keys and Wi-Fi passwords are
**never** carried across by any import — only re-pairing creates them.

## Connecting — Wi-Fi or USB

- **Wi-Fi (default):** the wheel is a **Wi-Fi access point only** —
  it creates its own `RSH` network and your PC joins it. The wheel
  never joins your home network. Stream traffic is UDP; control
  traffic is HMAC-authenticated. After pairing, the wheel locks the
  `RSH` network with WPA2 and the app rejoins it itself (see
  [Pairing](#pairing)).
- **USB-serial:** plug the wheel in with a **data** cable
  (charge-only cables do not work). The app auto-detects the COM port
  (COM10+ supported). Opening the port never resets the board.
- If the wheel is unreachable, the Dashboard shows the connection
  state and the Output page shows live axes/buttons only while a
  session is up. A failed `GET_CAPS` inside the pairing window may be
  retried; after the window closes, pairing is over until a fresh
  window is opened (pair button, USB, or reboot — see
  [Pairing](#pairing)).

## The 8 screens

| Screen | What you do there |
|---|---|
| **Dashboard** | Connection + device status, live input overview, game launcher |
| **Setup** | The first-run wizard (connect → pair → pins → calibrate → firmware) |
| **Tuning** | Per-module live tuning (steering, pedals, …) — see below |
| **Buttons** | Button / encoder mapping |
| **Output** | Output backend choice (HIDMaestro / vJoy), live axes/buttons, device state |
| **Profiles** | Game + car profiles, backup / restore |
| **Monitor** | Engine, transport and stream health (rate, loss, jitter) |
| **Settings** | Modules on/off, pins, system (firewall, autostart, tray, updates), diagnostics export, language (English / Urdu / Hindi), light / dark theme |

## Per-module tuning

Every input module has its own tuning card on the **Tuning** screen:
Steering, Accelerator, Brake, Clutch, Handbrake, Shifter, Horn,
Buttons, Encoders, LED.

- **Tuning shows a live On / Off / Unknown badge** per module. Tuning
  does **not** have an enable toggle — module on/off lives **only in
  Settings** (single source of truth), so a module you turn off in
  Settings is off everywhere.
- Tuning values (deadzone, sensitivity, ranges, …) apply live to the
  200 Hz engine pipeline. Changes are stored in the active profile.
- If a module is off, its tuning card shows it as off — it is not a
  bug and not a missing feature.

## Profiles

- **Profile v2** — one profile per game, with optional per-car
  overrides. When Assetto Corsa / ACC / iRacing is running, the app
  reads the game's telemetry (read-only shared memory) and switches
  automatically: game profile → car profile once the car is stable
  (4 polls / 2 s), back to the pre-game profile when the game closes.
  Manual override always wins; a failed switch leaves the current
  profile unchanged.
- **Backup / restore** — 3-generation backup
  (`current → .bak1 → .bak2`). If the profile store is corrupt at
  startup, the app fails closed (exits) and the backups survive —
  restore from Settings.
- Profiles live in `%AppData%\RSH`. Uninstalling the app does **not**
  delete them (they live outside the install directory).

## Game launcher

From the **Dashboard** you can launch a detected game (Steam library
scan + iRacing default path) and stop it again. Rules:

- **One game at a time.** Launching a second game is refused while
  one is tracked.
- **Stop kills only the tracked process** (the game exe you launched),
  never anything else.
- Detected games: Assetto Corsa, Assetto Corsa Competizione, iRacing,
  DiRT Rally 2.0. Other games are not in the launcher catalogue yet —
  see [GAME-MATRIX.md](GAME-MATRIX.md).
- **No game is verified to work with RSH in-game yet** (0 of 16
  backend–game pairs verified). The launcher launching a game does not
  mean the wheel is confirmed working in that game — that needs the
  in-game verification procedure in the game matrix.

## Output backend — HIDMaestro or vJoy

The Output page lets you choose how the wheel appears to games as a
DirectInput device. **Each backend presents a different device to the
game** — a game that has seen the vJoy wheel sees the HIDMaestro wheel
as a brand-new device, and its bindings must be made again from
scratch.

| Backend | Default? | Notes |
|---|---|---|
| **HIDMaestro** | **Yes (default)** | User-mode UMDF2 driver, installed by an elevated sidecar (`rsh-hm-sidecar.exe`, ships inside the installer). **Windows asks (UAC) on every launch while this backend is active** — that is expected, not an error. Driver install happens on first run (admin). |
| **vJoy** | Fallback | Rust FFI, **no UAC prompt** at runtime. **vJoy is not bundled** — install it yourself (one-time admin install) if you want this backend. If vJoy is missing, the Output page shows "vJoy not found — [Install vJoy] [Open Diagnostics]" instead of crashing or faking output. |

- If **neither** backend is available, output is disabled, the
  Dashboard warns, and the app keeps running.
- **No force feedback.** The wheel has no FFB motor, so no game's FFB
  features work on RSH — "FFB does not work" is the expected result
  for every game, never a failure.
- **XInput-only titles will not see an RSH wheel** on either backend
  (they only read Xbox-style controllers). That is a "not detected —
  XInput-only" result, not a bug.
- Handbrake is mapped to the Slider axis on either backend (analog).

## Updating

v1.0.1 is **check-only**: Settings can check for a new version
against the release `latest.json`, but the app cannot download or
install an update by itself. When an update ships, download the new
installer from GitHub Releases and install it over the old one —
profiles and settings survive (same `%AppData%\RSH`, same upgrade
code, single "Apps & features" entry). Firmware is never changed by
an app update, and the board rejects firmware downgrades.

## Troubleshooting

### The window is blank / a page shows no text

This is the lesson of the old C# app, and the Rust app is built
against it: **every screen renders literal text with a CSS fallback**,
so a missing theme or a failed binding can no longer produce a blank
window. If you still see a blank window or an empty page:

1. Check the app is actually the latest release (`RSH — Real Sim Hub
   v1.0.1` in the title bar). An older or mixed install can paint
   oddly — uninstall, then install the release installer cleanly.
2. Look at the log: `%AppData%\RSH\logs\rsh-YYYYMMDD.log` (today's
   date, no dashes). If that folder is empty or unwritable, the app
   falls back to `%TEMP%\RSH\logs`.
3. Export diagnostics from **Settings → Diagnostics** (redacted bundle:
   app version, OS, last 200 log lines — no settings, no keys, no
   passwords) and keep it for a bug report.
4. If a single page fails to load (e.g. "Dashboard could not be
   loaded"), the other pages still work — the failure is logged, not
   silent. Restart the app once; if the same page fails twice, export
   diagnostics and report it with the log lines.

### Wheel not found / will not connect

1. USB: use a **data** cable, try a rear USB port, check the COM port
   appears in Device Manager (CP210x / CH340 driver installed).
2. Wi-Fi: is your PC joined to the wheel's `RSH` network? The wheel
   is a Wi-Fi access point only — it never joins your home network,
   so "same home network" is not the check: look for the `RSH`
   network in your Wi-Fi list. If the wheel never appears, reboot it
   and check the `RSH` network shows up.
3. Firewall: on first run Windows asks to allow RSH through the
   firewall — allow it. Settings shows the firewall rule state
   (rule name `RSH`, inbound, program-scoped). If you declined it,
   allow RSH manually in Windows Firewall.
4. Pairing window: if you missed the window, open a fresh one — hold
   BOOT for 3 seconds on an unpaired board (firmware v4.0.2, 120 s
   window), plug in USB, or reboot the wheel (60 s boot window).
   See [Pairing](#pairing).

### "vJoy not found"

Install vJoy yourself (one-time admin install), then restart the app
and select vJoy on the Output page. HIDMaestro remains the default
and needs no vJoy.

### HIDMaestro UAC prompt every launch

Expected while HIDMaestro is the active backend — the sidecar must run
elevated for the whole session (the input shared section is read-only
to non-elevated processes). To stop the prompts, switch the Output
backend to vJoy.

### Factory reset (config gone wrong)

Try **FACTORY_RESET** from the app first (keeps your key + Wi-Fi
password). If the board misbehaves badly, hold **BOOT 5–10 s** for
safe mode (factory pin map + AP, no config applied), then send a good
config from the app. A bad config can never brick the board: on boot,
an unconfirmed config rolls back (Last-Known-Good, then factory
fallback). Full reflash is the last resort — see
[FLASH-GUIDE.md](FLASH-GUIDE.md).

### Where things live

| Thing | Location |
|---|---|
| App data (profiles, settings) | `%AppData%\RSH` |
| Logs | `%AppData%\RSH\logs\rsh-YYYYMMDD.log` (fallback `%TEMP%\RSH\logs`) |
| Secrets (device key) | Windows Credential Manager, service `RSH` — never in a file |
| Install | Current-user profile (NSIS) / WiX default (MSI) |
| Startup status | `%TEMP%\RSH\startup-status.txt` |

## Uninstall

Uninstall from "Apps & features" — the app and shortcut are removed.
Your profiles, settings and logs in `%AppData%\RSH` **survive** the
uninstall, so a reinstall finds them again. To wipe everything,
delete `%AppData%\RSH` yourself after uninstalling (this does **not**
delete the key from Credential Manager — remove the `RSH` entry there
too if you want the pairing gone; the board still holds its key until
you KEY_RESET it).
