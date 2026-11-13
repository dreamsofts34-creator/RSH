# RSH — ESP32 Flashing Guide (step by step)

First flash is via USB. Every later update is wireless (Web OTA page or the app wizard) — no cable needed again.

## What flashes
- File: `RSH-Firmware-DevKitV1-v3.3.bin` (build it from `firmware/rsh/`; not included in the cleaned package).
- It is a **merged** image (bootloader + partitions + app), so it always flashes at address **0x0**.

## You need
- ESP32 DevKit V1 board and a **data** USB cable (charge-only cables never show a COM port).
- Chrome or Edge (the web flasher needs Web Serial; Firefox will not work).

## Steps
1. **Plug the board in.** Open Device Manager → *Ports (COM & LPT)*. A COM port should appear (CP210x or CH340 name). If none: change the cable first, then install the CP2102/CH340 USB driver for your board's chip.
2. **Open the flasher:** `adafruit.github.io/Adafruit_WebSerial_ESPTool` — set baud **921600** (fall back to 115200 if flashing fails).
3. **Connect.** Click *Connect*, pick the board's COM port. If you get "Failed to connect": hold the board's **BOOT** button, click Connect, release BOOT once "Connecting..." starts (on some boards also tap **EN/RST** while holding BOOT).
4. **Give the file.** In the **first (top) row only**, click *Choose a file...* and select `RSH-Firmware-DevKitV1-v3.3.bin`. Leave its offset at **0**. Leave the other three rows empty.
5. **Program.** Click *Program*. Erasing → Writing runs (~30–60 s). Do not touch the cable or board.
6. **Restart & confirm.** When done, press **EN/RST** on the board (or replug USB). Check the Wi-Fi list on PC/phone: a network named **RSH** should appear — that confirms the firmware is running. The Wi-Fi password is in the Essentials README / `config.h`.

## If something goes wrong
- **No COM port:** data cable → driver (CP2102/CH340) → another USB port (rear ports are more stable).
- **Repeated connect failures:** BOOT-button trick + baud 115200.
- **Flashed but no RSH Wi-Fi:** press EN/RST; if still nothing, reflash and double-check the offset is 0.
- **Flash stalls mid-way:** change USB port/cable and retry; nothing is damaged — the board only boots a complete image.

## Later updates (no cable)
With v3.3 running, upload a new app `.bin` either from the PC app's setup wizard (Firmware step) or in a browser on the board's update page at `192.168.4.1` while connected to the RSH Wi-Fi (login details in the Essentials README / `config.h`). Dual-partition OTA keeps the previous image as a fallback if an update is interrupted.
