# RSH — Real Sim Hub

[![Release](https://img.shields.io/github/v/release/dreamsofts34-creator/RSH?label=release&color=2F6BFF)](https://github.com/dreamsofts34-creator/RSH/releases/latest)
[![CI](https://github.com/dreamsofts34-creator/RSH/actions/workflows/ci.yml/badge.svg)](https://github.com/dreamsofts34-creator/RSH/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)
[![Windows 10 / 11](https://img.shields.io/badge/platform-Windows%2010%20%7C%2011-blue.svg)](https://github.com/dreamsofts34-creator/RSH/releases/latest)

RSH connects an ESP32 sim-racing wheel to Windows racing games over Wi-Fi or USB.
The wheel's steering, pedals, shifter, buttons and encoders become a Windows game controller,
so you can drive with your own hardware in games like Assetto Corsa, ACC and iRacing.
A small backend engine runs in the background; the app window is only there when you open it.

## Install

### 1. Board firmware

**Supported board:** ESP32 DevKit V1.

- **First time, or a board that won't boot:** flash the merged image `RSH-Firmware-DevKitV1-v4.0.4.bin` at offset `0x0` with esptool:

  ```sh
  esptool.py --port <port> write_flash 0x0 RSH-Firmware-DevKitV1-v4.0.4.bin
  ```

- **Updating a working board:** use the app — Settings → Firmware → update (WebOTA). The image is signature-checked before it is applied. Update the app before the firmware.

Forgot the Wi-Fi password? Hold the board's BOOT button for 10+ seconds — it resets to an open hotspot.

### 2. Software setup

1. Download **RSH_2.0.10_x64-setup.exe** from the [latest release](https://github.com/dreamsofts34-creator/RSH/releases/latest) and run it. If RSH is already installed, it offers an update instead.
2. Open RSH and follow the **Setup wizard**: connect the board, check drivers, calibrate, add a game.
3. **First connect:** join the board's open hotspot (`RSH-XXXX`) from the wizard, or plug in the USB cable. No pairing, no code.
4. **Set a Wi-Fi password** in Settings → Connection. Until you do, the hotspot stays open.
5. Add your games on **Overview**, turn **Send to game** on, and drive.

## Notes

- Windows 10 22H2 / 11, x64 only. The app is unsigned, so Windows SmartScreen will ask — choose "Run anyway".
- Output is vJoy by default (HIDMaestro optional). No force feedback yet — planned for a future update.
- Games are added by hand (name + exe or shortcut). RSH never scans for games.
- No auto-update: new versions come from [GitHub Releases](https://github.com/dreamsofts34-creator/RSH/releases). Firmware updates come from the app (WebOTA, signature-checked).
- The backend engine keeps running when the window is closed. Closing the window never stops the wheel.

## For developers

```sh
cargo test            # library crates
cargo clippy --all-targets -- -D warnings
cargo fmt --check
pio run -e esp32dev   # firmware, from firmware/
```

Tests run in the Virtual Test Rig (`tools/rsh-rig`). Docs: [Changelog](docs/CHANGELOG.md) · [Architecture](docs/ARCHITECTURE.md) · [Protocol v2.2](docs/PROTOCOL-V2.2.md) · [UI design](docs/UI-DESIGN.md)

## License

MIT — see [LICENSE](LICENSE). Third-party licenses: [THIRD-PARTY-LICENSES.md](THIRD-PARTY-LICENSES.md).
