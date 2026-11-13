# RSH — ESP32 Flashing Guide (step by step)

**Version 2.0.0 — app v2.0.0 · firmware v4.0.0 · protocol v2.1**

First flash is via USB. Every later update is wireless (Web OTA page or the app wizard) — no cable needed again.

## What flashes
- File: `RSH-Firmware-DevKitV1-v4.0.0.bin` (build it from `firmware/rsh/`; not included in the cleaned package).
- It is a **merged** image (bootloader + partitions + app), so it always flashes at address **0x0**.

## You need
- ESP32 DevKit V1 board and a **data** USB cable (charge-only cables never show a COM port).
- Chrome or Edge (the web flasher needs Web Serial; Firefox will not work).

## Steps
1. **Plug the board in.** Open Device Manager → *Ports (COM & LPT)*. A COM port should appear (CP210x or CH340 name). If none: change the cable first, then install the CP2102/CH340 USB driver for your board's chip.
2. **Flash the merged bin — pick one method:**
   - **A (recommended): esptool.py** (same tool as `docs/BRICK-RECOVERY.md`):
     ```bash
     esptool.py --chip esp32 --port COM3 --baud 921600 write_flash 0x0 RSH-Firmware-DevKitV1-v4.0.0.bin
     ```
     Linux par port `/dev/ttyUSB0` hota hai. Baud fail ho to `115200` try karo.
     Offset **0x0** hona chahiye.
   - **B: PlatformIO** (from `firmware/rsh/`):
     ```bash
     pio run -t upload
     ```
   - **C (alternative): Adafruit WebSerial ESPTool** (`adafruit.github.io/Adafruit_WebSerial_ESPTool`, Chrome/Edge — Web Serial chahiye, Firefox nahi chalega): Connect, pick the board's COM port, baud **921600** (fall back to 115200 if flashing fails). If you get "Failed to connect": hold the board's **BOOT** button, click Connect, release BOOT once "Connecting..." starts (on some boards also tap **EN/RST** while holding BOOT).
3. **Give the file (method C only).** In the **first (top) row only**, click *Choose a file...* and select `RSH-Firmware-DevKitV1-v4.0.0.bin`. Leave its offset at **0**. Leave the other three rows empty.
4. **Program (method C only).** Click *Program*. Erasing → Writing runs (~30–60 s). Do not touch the cable or board. (Methods A/B finish on their own — esptool aakhir mein `Hash of data verified` likhta hai.)
5. **Restart & confirm.** When done, press **EN/RST** on the board (or replug USB). Check the Wi-Fi list on PC/phone: a network named **RSH** should appear — that confirms the firmware is running.
6. **Pair immediately — first pairing is over an OPEN network (read this).** The board's Wi-Fi password is per-device and random — there is no default password. Before the first pairing the `RSH` network is **open (no password)** so the app can connect and receive the key + password in the first HELLO (pairing window: 60 s from AP-up; PROTOCOL-V2.1 §4.2, OPEN-6 — CLOSED decision). **Security warning (TM-W02 residual risk):** during that open window there is no link-layer encryption — anyone in Wi-Fi range could listen to the pairing exchange. So: **pair at home, in your own room, immediately after flashing — do not leave the board unpaired.** As soon as pairing succeeds (first HMAC-signed GET_CAPS), the AP switches to WPA2 with the per-device password and the key is never sent again. If the 60 s window expires before you pair, the AP stays visible but pairing is refused (HELLO sends a zero key) — re-pairing then needs KEY_RESET (hold BOOT ≥ 10 s, then pair again). If you ever suspect the pairing was overheard, do a KEY_RESET for a fresh key + password. After pairing, the Wi-Fi password lives in the app (DPAPI KeyStore) — never write it down in chat or docs.

## If something goes wrong
- **No COM port:** data cable → driver (CP2102/CH340) → another USB port (rear ports are more stable).
- **Repeated connect failures:** BOOT-button trick + baud 115200.
- **Flashed but no RSH Wi-Fi:** press EN/RST; if still nothing, reflash and double-check the offset is 0.
- **Flash stalls mid-way:** change USB port/cable and retry; nothing is damaged — the board only boots a complete image.

## Later updates (no cable)
With v4.0.0 running, upload a new app `.bin` either from the PC app's setup wizard (Firmware step) or in a browser on the board's update page at `192.168.4.1` while connected to the RSH Wi-Fi (login: user `admin`, password = board ka per-device password). **Sirf signed official release upload karo** — board image ki Ed25519 signature verify karta hai (docs/SIGNED-OTA.md); unsigned ya purani image reject ho jati hai. Dual-partition OTA keeps the previous image as a fallback if an update is interrupted. Update ke baad app GET_CAPS se naya version confirm karti hai — confirm na ho to update kamyab nahi mani jati.
