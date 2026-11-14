# RSH — Flash Guide (firmware v4.0.1)

**Board:** ESP32 DevKit V1 (original ESP32, 4 MB flash) ·
**Firmware:** C++ v4.0.1 · **Protocol:** v2.1 ·
**App:** Rust v1.0.0

> **Do you need this guide at all?** Probably not. The Rust app
> v1.0.0 talks to the same C++ v4.0.1 board — **if your wheel already
> runs firmware v4.0.1, no reflash is needed** to use the Rust app
> (you only need to [re-pair](USER-GUIDE.md#re-pairing-after-the-c-rust-switch-mandatory)).
> This guide is only for a **new/blank board** or a **board that will
> not boot**. The detailed companion (signing internals, source build,
> host-mock checks) is [REFLASH-GUIDE.md](REFLASH-GUIDE.md).

## 1. Before you start — which file?

| Situation | Action |
|---|---|
| Rust app v1.0.0 install / update | **Do NOT reflash** — an app update never changes firmware |
| New (blank) ESP32 DevKit V1 | Flash the merged factory image (§3) |
| Board seems bricked (no Wi-Fi, will not boot) | Flash the merged factory image (§5 recovery) |
| Config broken / board behaving oddly | Try app **FACTORY_RESET** first (keeps key + Wi-Fi password); or BOOT-hold safe mode (§4) — **not** a reflash |
| Reflash v4.0.1 → v4.0.1 again | No-op — only if the image looks corrupt |

**Use the merged factory image only:**

| File | Size | Use |
|---|---|---|
| `RSH-Firmware-DevKitV1-v4.0.1.bin` (merged) | **955,984 B** | ✅ USB flashing — this is the file for this guide |
| `firmware.bin` (app-only) | 890,448 B | ❌ **Never** for USB flashing / recovery — it has no bootloader and will not boot |
| `firmware-signed.bin` | 890,516 B (app + 68 B trailer) | ❌ **OTA only** (WebOTA / app upload) — not for USB flashing |

The merged image contains bootloader (`0x1000`) + partitions (`0x8000`)
+ boot_app0 (`0xe000`) + app (`0x10000`) in one file, flashed at
**offset `0x0`**. A wrong offset or the wrong file means the board will
not boot — check the size before flashing.

**Signed firmware only.** Every firmware image is Ed25519-signed
(64-byte signature + 4-byte magic `RSHS` trailer, 68 B total). The
board verifies the signature **before applying** any image, and
rejects unsigned, tampered, or **downgrade** images — the board will
never run an older firmware than the one it already has. There is no
way to force an unsigned or older image onto the board, by USB or by
OTA.

## 2. What you need

| Item | Where from |
|---|---|
| Merged factory bin `RSH-Firmware-DevKitV1-v4.0.1.bin` (**955,984 B**) | GitHub Release v1.0.0 assets, or a local `pio run` (`.pio/build/esp32dev/`) |
| `SHA256SUMS` (from the same release) | GitHub Release assets — verify before flashing (§3, step 1) |
| `esptool.py` | With PlatformIO (`tool-esptoolpy`), or `pip install esptool` |
| USB **data** cable (charge-only will not work) | — |
| CP210x / CH340 USB-serial driver (match your board's chip) | Vendor site / OS update |
| PC (Windows or Linux) | — |

## 3. Flashing (new board or re-flash)

1. **Verify the file first.**
   ```powershell
   # Windows (PowerShell)
   Get-FileHash RSH-Firmware-DevKitV1-v4.0.1.bin -Algorithm SHA256
   ```
   ```bash
   # Linux
   sha256sum RSH-Firmware-DevKitV1-v4.0.1.bin
   ```
   Compare with `SHA256SUMS` from the release, and check the size is
   **955,984 B**. A hash mismatch or a smaller file (e.g. 890,448 B
   `firmware.bin`) means the wrong file — **do not flash it**,
   download it again.
2. **Connect the board by USB.** It must appear as a COM port
   (Windows Device Manager) or `/dev/ttyUSB*` (Linux). If it does not:
   change the cable, install the driver, try a rear USB port.
3. **Download mode:** hold **BOOT**, press-and-release **EN/RST** once,
   then release BOOT. (On some boards, BOOT-hold + plug-in is enough.)
4. **Flash** (put in your own port — `COM3`, `COM4`, … on Windows):
   ```bash
   esptool.py --chip esp32 --port COM3 --baud 921600 \
     write_flash 0x0 RSH-Firmware-DevKitV1-v4.0.1.bin
   ```
   ```bash
   # Linux example
   esptool.py --chip esp32 --port /dev/ttyUSB0 --baud 921600 \
     write_flash 0x0 RSH-Firmware-DevKitV1-v4.0.1.bin
   ```
   If the baud rate fails, retry at `115200`. **Offset `0x0` is
   mandatory** — any other offset and the board will not boot.
5. **Verify:** esptool ends with `Hash of data verified`. If it does
   not, flash again (try another cable/port first).
6. **Restart:** press EN/RST or unplug/replug USB.
7. **Confirm:** the **`RSH`** Wi-Fi network appears on your PC/phone —
   the firmware is running. Then connect from the Rust app: it reports
   firmware **v4.0.1** + protocol **v2.1**. First pairing after a fresh
   flash (or after `erase_flash`) must happen inside the **60-second**
   pairing window after the SoftAP comes up — see
   [USER-GUIDE.md](USER-GUIDE.md#pairing-60-second-window).

## 4. BOOT-hold safe mode & key reset (no reflash needed)

Safe mode is detected **at runtime** (the loop reads GPIO0 / BOOT every
iteration — not only at power-up):

| Hold | Result |
|---|---|
| 5 s ≤ hold < 10 s | **Safe mode** — factory pin map + AP; no config is applied (stage + commit are blocked with `BUSY (7)`) |
| hold ≥ 10 s | **Key-reset-armed** — only in this state does `KEY_RESET` (opcode `0x0B`) run: the board generates a fresh key + fresh Wi-Fi password, and the old key stops working immediately |

- **Bad config can never brick the board.** A reboot never confirms an
  unconfirmed config — on boot it rolls back (Last-Known-Good, then
  factory fallback). Reboot, use safe mode or the fallback config, then
  send a good config from the app.
- In safe mode only AP + WebOTA run — a signed `firmware-signed.bin`
  can also be uploaded by WebOTA from safe mode (the board still
  verifies before applying; unsigned images are rejected).
- After a **KEY_RESET** you must **re-pair** from the app within the
  60-second window (the new key is disclosed exactly once).

## 5. Recovery — when flashing fails

The board is effectively never bricked: the ESP32 ROM bootloader
always survives, so a USB re-flash of the factory image always brings
it back.

| Problem | What to do |
|---|---|
| No COM port / device not seen | Cable → driver → USB port (try rear ports) |
| `Failed to connect` | BOOT-button trick (§3 step 3) + retry at baud `115200` |
| Flash stalls mid-way | Change cable/port, flash again — a partial flash cannot damage the board (bootloader is in ROM) |
| Flashed, but no `RSH` Wi-Fi | Press EN/RST; still nothing → check the offset is `0` and the file is the **merged** bin, not `firmware.bin` |
| Everything fails | **Last resort:** `esptool.py erase_flash`, then §3 again. This wipes everything including NVS/config — the board returns to factory state and **pairing must be done again** |

## 6. After flashing — checklist

- [ ] `RSH` Wi-Fi network is visible
- [ ] Rust app v1.0.0 connects (HELLO OK)
- [ ] App shows firmware **v4.0.1** + protocol **v2.1**
- [ ] Output page shows live axes/buttons (200 Hz stream)
- [ ] First pairing done inside the 60-second window (new board /
      `erase_flash` only)

## 7. For developers — building from source

```bash
export TMPDIR=~/tmp   # /tmp is a 512 MB tmpfs — large builds need this
cd <firmware copy>    # never build inside reference/ — use a scratch copy
pio run               # output: .pio/build/esp32dev/RSH-Firmware-DevKitV1-v4.0.1.bin (955,984 B)
pio run -t upload     # board must be in download mode (§3 step 3)
```

Signing (release only): with `RSH_SIGNING_KEY=<path to the Ed25519
private-key PEM>` set, the build produces `firmware-signed.bin`
(`firmware.bin` + 68 B trailer). Key unset → signing skipped (dev build
stays unsigned) and any stale signed bin is deleted. Key set but
missing / sign fails → the build fails hard. The private key never
enters the repo — it lives in `signing-keys/` outside the repo (0600)
and in the CI secret `RSH_SIGNING_KEY_PEM`; public-key fingerprint
(SHA-256) `387764a4529337c437b233a148b48f9b2ffe3f3b5015e14c95737cb3a67a6411`.
Never print, copy, or share the key — use only its path. The Tauri
updater keypair is a **separate** keypair; firmware key, updater key,
and the (deferred) OV code-signing certificate are three different
things — see [KEY-LIFECYCLE-RUST.md](KEY-LIFECYCLE-RUST.md).
