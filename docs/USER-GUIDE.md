# ApexLink — User Guide

Wireless DIY sim-racing wheel: an ESP32 in the wheel sends its movement over
its own WiFi; ApexLink on the PC turns that into a standard game controller
(a vJoy virtual joystick) that every racing game understands.

**Principle:** the firmware stays dumb and fast (sample → send, 200 times a
second). Everything smart — calibration, smoothing, curves — happens here,
where you can change it without reflashing anything.

---

## 1. What you need

- The wheel, flashed with the ApexLink firmware (ESP32 DevKit V1 build).
- Windows 10 or 11 (64-bit).
- The **vJoy driver, BrunnerInnovation v2.2.2.0** — this is the signed
  release that works on Windows 11. Get the driver and `vJoyInterface.dll`
  from the same release; mixing versions does not work (ApexLink checks and
  tells you if they don't match).
- A game. Any game that accepts a standard joystick works.

## 2. Install the vJoy driver (once, as administrator)

1. Run the vJoy 2.2.2.0 installer (it needs admin — it's a driver).
2. Open **vJoyConf** (Configure vJoy).
3. Select device **1**. Enable axes **X**, **Y** and **Z**. Set the number
   of buttons to at least 2 (32 gives you room for the future).
4. Click **Apply**. Leave "Enable vJoy" ticked.
5. Optional check: open **joy.cpl** (Set up USB game controllers) — you
   should see a vJoy device listed.

ApexLink itself finds `vJoyInterface.dll` automatically: it looks next to
the app first, then in the vJoy install folder
(`C:\Program Files\vJoy\x64`). No manual copying needed.

> Memory Integrity / HVCI note: if Windows blocks the driver, the Output
> page in ApexLink will say the driver is not enabled. This is a Windows
> policy on your machine, not an ApexLink fault.

## 3. Install ApexLink

Run **ApexLink-Setup-1.0.0.exe**. It installs per-user (no admin) into
`%LOCALAPPDATA%\Programs\ApexLink` and can add a desktop shortcut and
Windows autostart. A portable build (just run `ApexLink.exe` from the
folder) works exactly the same.

Your settings and profiles live in `%APPDATA%\ApexLink` and survive
updating — uninstall/reinstall never loses your calibration.

## 4. First run — the setup wizard

The wizard opens automatically the first time:

1. **vJoy check** — it turns green when the driver is feeding. If it stays
   red, the message on screen says exactly why (not installed, wrong
   version, device busy in another app, device not enabled in vJoyConf).
2. **Connect to the wheel** — power the wheel and join this PC's WiFi to
   the **APEXLINK** network. The page goes green when packets arrive
   (you should see ~200 packets/sec).
   *While you race you are on the wheel's network, so this PC has no
   internet on that adapter. That is expected.*
3. **Calibrate steering** — capture the centre, then the full range.
4. **Calibrate pedals** — capture released, then fully pressed.

You can re-run the wizard any time from **Settings → Run setup wizard**,
and fine-tune later on the Calibration page.

## 5. Everyday use

- Start ApexLink (or let autostart do it). It lives in the system tray;
  closing the window hides it there. Exit fully from the tray menu.
- The **Dashboard** shows the live wheel, pedals, link rate and whether
  the output is being fed. The toggle there pauses output instantly.
- **Safety watchdog:** if the wheel goes silent for 250 ms (out of range,
  battery, crash), the output snaps to neutral — centred steering, pedals
  off, no buttons — and recovers by itself when data returns. Your car
  never keeps "holding" a stuck input.
- **Profiles:** one per game. Calibration, processing and button mapping
  all live in the profile; switching is instant, even mid-session.
- **Processing defaults that work:** deadzone 2%, curve 0%, smoothing 30%.
  Set every in-game steering deadzone to **zero** and linearity to 100% —
  ApexLink does that job once, properly.

### In your game

1. Make sure joy.cpl shows the vJoy axes moving (steering = X,
   accelerator = Y, brake = Z).
2. Bind: steering → vJoy X, throttle → vJoy Y, brake → vJoy Z
   (separate axes), gear up/down → the vJoy buttons you chose on the
   Buttons page (defaults 1 and 2).
3. Set the in-game steering lock/range to your wheel's real physical
   range, deadzone 0, linearity 100%.
4. Old game that only offers one shared throttle/brake axis? Turn on
   **Processing → Combine pedals** for that game's profile.

## 6. Troubleshooting

| Symptom | Likely cause | Fix |
|---|---|---|
| Dashboard: "Wheel not connected" | PC not on APEXLINK WiFi, wheel off, or HELO blocked | Join APEXLINK, check wheel LED (solid = connected), check firewall allows ApexLink |
| Packets arrive but stop after 2 s | Windows dropped the WiFi (power saving) | In Device Manager, disable "allow the computer to turn off this device" for the WiFi adapter |
| Output red: "device BUSY" | Another feeder app holds the vJoy device | Close the other app (or pick a different device on the Output page) |
| Output red: "device missing" | Device not enabled in vJoyConf | vJoyConf → enable device 1 with X/Y/Z → Apply |
| Output red: DLL/driver version mismatch | vJoy pieces from different releases | Reinstall vJoy 2.2.2.0 completely |
| Steering drifts/jitters at centre | Sensor noise | Raise smoothing a little, or deadzone to 3–4% |
| Steering feels floaty | Too much smoothing | Lower smoothing toward 15–20% |
| Car pulls to one side | Centre moved (pot shifted) | Re-capture centre on the Calibration page |
| Pedals never reach 100% / always on | Range changed | Re-run pedal capture |
| High packet loss % on Diagnostics | WiFi congestion/interference | Move closer; firmware v3 auto-picks the quietest channel at boot — restart the wheel |

Something else? Open **Diagnostics → Export diagnostics (.zip)** and keep
the file — it contains the logs and the counters that explain what the
link was doing.

## 7. Performance (by design)

ApexLink is built to disappear on low-end PCs:

- The 200 Hz pipeline allocates **zero bytes per tick** on its hot path
  (measured: ~0.45 µs of work per tick).
- Idle: ~1 keepalive packet per second, UI asleep, engine waiting.
- The interface redraws at 30 Hz and only for the page you are looking
  at; graphs pause when you leave the page or minimise.
- Budgets it is held to: ≤1% CPU idle, ≤3% while racing, ≤120–150 MB
  working set. The bundled `ApexLink.Benchmarks` tool prints the real
  numbers on your machine.

## 8. Files and folders

| What | Where |
|---|---|
| App | `%LOCALAPPDATA%\Programs\ApexLink` |
| Settings | `%APPDATA%\ApexLink\settings.json` |
| Profiles | `%APPDATA%\ApexLink\profiles.json` (a `.bak` is kept automatically) |
| Logs | `%APPDATA%\ApexLink\logs` (daily, 7 days kept) |

Settings and profiles are saved atomically with a backup — a crash or
power cut mid-save cannot corrupt them; on any read failure ApexLink
falls back to the backup, then to sane defaults. It never crashes because
of a broken config file.
