# RSH — Real Sim Hub

[![Release: v2.0.6](https://img.shields.io/github/v/release/dreamsofts34-creator/RSH?label=release&color=2F6BFF)](https://github.com/dreamsofts34-creator/RSH/releases/tag/v2.0.6)
[![CI](https://github.com/dreamsofts34-creator/RSH/actions/workflows/ci.yml/badge.svg)](https://github.com/dreamsofts34-creator/RSH/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)
[![Platform: Windows 10 / 11](https://img.shields.io/badge/platform-Windows%2010%20%7C%2011-blue.svg)](https://github.com/dreamsofts34-creator/RSH/releases/latest)
[![Firmware: v4.0.3](https://img.shields.io/badge/firmware-v4.0.3-green.svg)](docs/ARCHITECTURE.md)
[![Protocol: v2.2](https://img.shields.io/badge/protocol-v2.2-green.svg)](docs/PROTOCOL-V2.2.md)

**RSH (Real Sim Hub)** connects an ESP32 sim-racing wheel to Windows
racing games over Wi-Fi or USB-serial. The PC app is **Rust + Slint**:
a small always-on daemon does the work (engine, transport, output,
tray), and a Slint window shows it only when you open it. The wheel
firmware is **C++ v4.0.3** and the wire protocol is **v2.2 (frozen)**.

> **A full Rust recreation, fresh start — not a port of the C# app.**
> The C# series (v2.0.4 and earlier) is superseded. **There is no
> pairing anymore (v2.0.6):** the board opens an unsecured hotspot,
> the PC joins it, the app connects — and you set a Wi-Fi password
> in Settings afterwards. Upgrading from an older release: **update
> the app first (v2.0.6), then flash firmware v4.0.3 from the app
> (WebOTA)**. Wheels still on firmware v4.0.2 keep working with app
> v2.0.6 (the old HELLO still parses) and the app offers the WebOTA
> update; an old app (≤ 2.0.5) cannot talk to firmware 4.0.3.

## Features

- **200 Hz wheel engine** — steering, pedals (accel / brake /
  clutch), handbrake, shifter, buttons, encoders, horn, LED strip,
  all processed at 200 Hz with per-module tuning.
- **Wi-Fi (UDP) + USB-serial** transports, plain control channel
  (protocol v2.2, reqId dedup, no HMAC, no pairing) — the Wi-Fi
  password you set in Settings is the only boundary.
- **Output backends** — vJoy (default) and HIDMaestro (elevated
  sidecar, started lazily only when "Send to game" is on). No
  force feedback — the wheel has no FFB motor.
- **Per-game profiles** with automatic switching from Assetto Corsa
  / ACC / iRacing telemetry (read-only shared memory). Games are
  added manually (name + exe/shortcut) and launched locally — there
  is no auto-detection or scanning.
- **Live everything** — steering and pedal graphs update live at
  20 Hz; every action shows a loading state with a timeout and a
  plain-language error instead of hanging.
- **3 screens** — Overview, RSH Wheel (Steering / Pedals / Buttons /
  Extras), Settings — dark theme, 920×580, English only. See
  [UI design](docs/UI-DESIGN.md).
- **Signed firmware only** — Ed25519-signed OTA images,
  verify-before-apply, anti-downgrade. The app itself ships
  **unsigned** (OV code-signing certificate deferred — expect a
  Windows SmartScreen warning; choose "Run anyway").

## Install

1. Download **`RSH_2.0.6_x64-setup.exe`** from the
   [v2.0.6 GitHub Release](https://github.com/dreamsofts34-creator/RSH/releases/tag/v2.0.6).
2. Run the installer — it asks for admin **once**, at install time,
   to create the Windows Firewall rule for the daemon. If RSH is
   already installed, the installer offers an **update** instead
   (settings and profiles are kept).
3. Launch RSH and run the **Setup wizard**:
   **Connect** (Wi-Fi or USB — Next unlocks when the board is
   reachable) → **Drivers** (vJoy check/install) →
   **Calibrate** → **Games**.
4. Pick your game controller on the **Settings** page (vJoy
   default), add your games on **Overview**, and launch a game.
   Flip **Send to game** on to drive.

### Connecting

- **First connect:** the board opens an open hotspot named
  `RSH-XXXX` (last 4 hex of the board's Wi-Fi MAC, e.g.
  `RSH-9A55`). Join it from the wizard (or plug in the USB
  cable) — the wizard checks the board is really reachable
  before it lets you continue. No pairing, no code, no window.
- **Set a password:** right after first connect, set a Wi-Fi
  password in **Settings → Connection** (8–63 printable ASCII).
  The board's AP then runs WPA2 with that password, and the same
  password is the WebOTA password. Until you set one, the AP is
  open and WebOTA is disabled — set a password promptly.
- **Forgot the password?** Hold the board's BOOT button for 10+ s
  — the firmware clears the password itself and reboots to an
  open AP. Set a fresh password afterwards.
- The Wi-Fi password is stored in Windows Credential Manager
  (service `RSH`). Secrets never live in files or logs.

## Firmware

- **Current firmware: v4.0.3** (ESP32 DevKit V1). The v2.0.6
  release ships two firmware files:
  - `RSH-Firmware-DevKitV1-v4.0.3.bin` — merged
    full-flash image, for a new board or a board that will not
    boot (flash at offset `0x0`, e.g. with esptool).
  - `RSH-Firmware-DevKitV1-v4.0.3-OTA.bin` —
    Ed25519-signed OTA image. The app updates the board from
    here (WebOTA, signature verified before applying).
- **Wheels already on v4.0.2 need no reflash to keep working** —
  app v2.0.6 still talks to them and offers the WebOTA update to
  4.0.3. Update the **app first**, then the firmware (a 4.0.3
  board's first boot deletes the old pairing secrets and brings
  the AP up open — set a fresh password after the flash). See
  [docs/CHANGELOG.md](docs/CHANGELOG.md) for the firmware version history.
- Firmware images are **signed only** — unsigned or downgrade
  images are rejected by the board.

## Architecture (summary)

```
ESP32 wheel (C++ firmware v4.0.3, protocol v2.2 frozen)
        │  Wi-Fi UDP 4210/4211/4212  or  USB-serial
        ▼
┌──────────────────────────────────────────────────────┐
│  rsh-daemon (Rust, always on — engine + transport +  │
│  output + tray + auto-profiles + local IPC server)   │
│        ▲  named pipe / local socket, JSON lines      │
│  rsh-ui (Slint window, only when opened)             │
└──────────────────────────────────────────────────────┘
        │  vJoy FFI  or  HIDMaestro sidecar (elevated)
        ▼
   Windows game controller → racing game
```

- **OS floor:** Windows 10 22H2 (build 19045) and Windows 11,
  x64 only. ARM64 is unsupported.
- **Data directory:** `%AppData%\RSH` (profiles, settings, logs
  `logs\rsh-YYYYMMDD.log`). Uninstall keeps it — delete it
  manually for a fully clean start.
- **Auto-update is off** — download new releases manually from
  GitHub Releases. Firmware updates come from the app (WebOTA).
- **Daemon:** runs from login, keeps running when the window is
  closed. Closing the window never stops the wheel or the output.
- Full architecture: [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)
  · UI design: [docs/UI-DESIGN.md](docs/UI-DESIGN.md) ·
  Changelog: [docs/CHANGELOG.md](docs/CHANGELOG.md)

## Repo tree

```
.
├── Cargo.toml / Cargo.lock     Rust workspace (Rust 1.100.0)
├── crates/                     14 crates (api, app-core, daemon,
│                               engine, ipc, led, output,
│                               output-hidmaestro, profiles,
│                               protocol, security, telemetry,
│                               transport, ui)
├── firmware/                   C++ ESP32 firmware v4.0.3
│                               (PlatformIO, esp32dev)
├── tools/                      rsh-rig (Virtual Test Rig),
│                               gate-checks, rsh-benchmarks,
│                               rsh-hm-sidecar (C# NativeAOT)
├── docs/                       docs (changelog, architecture, security, UI design)
├── src-tauri/ + frontend/      v1.x Tauri shell — kept but
│                               unshipped (CI still builds it)
├── .github/workflows/          ci.yml, release.yml
├── LICENSE                     MIT
└── THIRD-PARTY-LICENSES.md     license inventory + policy
```

## Development

```sh
cargo build                    # library crates (Linux OK)
cargo test                     # library crates
cargo clippy --all-targets -- -D warnings
cargo fmt --check
cargo bench -p rsh-benchmarks  # criterion benches

# Windows only:
cargo run -p rsh-ui            # run the Slint UI
cargo run -p rsh-daemon        # run the daemon

# Firmware (needs PlatformIO):
pio run -e esp32dev            # from firmware/
```

- Every test runs in the **Virtual Test Rig**
  (`tools/rsh-rig`) — ADC noise, Wi-Fi jitter/loss/reorder,
  clock drift, fuzz, soaks, fault injection. Real-board tests
  happen only at the final production release.
- The Tauri shell (`src-tauri`) is a workspace member but only
  compiles where the system webview libraries exist — on a Linux
  sandbox it does not (no webkit2gtk dev libraries). That is a
  Windows-only verification gap, not a code defect.
- Rust 1.100.0 stable (`rust-toolchain.toml`), `cargo fmt` +
  `cargo clippy -- -D warnings` clean, tests for new behaviour.

## Documentation

| Doc | What it is |
|---|---|
| [docs/CHANGELOG.md](docs/CHANGELOG.md) | Release history (app v2.0.6, firmware v4.0.3) |
| [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) | Daemon + UI split, IPC, connection state machine, threads |
| [docs/UI-DESIGN.md](docs/UI-DESIGN.md) | Approved UI design (layout, colors, screens, states) |
| [docs/PROTOCOL-V2.2.md](docs/PROTOCOL-V2.2.md) | Wire protocol v2.2 (HELLO, control, reqId dedup, SET_PASSWORD, WebOTA) |
| [docs/THREAT-MODEL-RUST.md](docs/THREAT-MODEL-RUST.md) | STRIDE threat model |
| [docs/SECURITY-INPUT.md](docs/SECURITY-INPUT.md) | Security inputs the threat model traces to |
| [THIRD-PARTY-LICENSES.md](THIRD-PARTY-LICENSES.md) | License inventory (no GPL/AGPL/LGPL anywhere) |

## Release status (v2.0.6, 2026-11-20 — in progress, not released)

- **v2.0.5 is the current live release.** **v2.0.6 removes pairing
  entirely** (open-AP first connect, Wi-Fi password in Settings,
  protocol v2.2, firmware v4.0.3) — it is in development on the
  `no-pairing-*` branches and is **not released yet**. See
  [CHANGELOG](docs/CHANGELOG.md).
- **Still to prove on a real Windows PC:** setup wizard,
  tray popup, connect-play / password-set / BOOT-hold reset /
  WebOTA on the real board, vJoy, smart installer, game
  end-to-end. No game is claimed to work until it is verified
  in-game on that backend.
- **Known open items:** pid.codes PR for VID `0x1209` / PID
  `0x5253` (not yet merged); OV code-signing certificate
  deferred — releases ship unsigned (SmartScreen warning
  expected); secure firmware image (flash encryption) is
  not yet board-verified.

## Contributing

- Rust 1.100.0 stable (`rust-toolchain.toml`), `cargo fmt` +
  `cargo clippy -- -D warnings` clean, tests for new behaviour.
- No fake implementations: unimplemented commands stay
  `NOT_YET_IMPLEMENTED`, unmeasured claims stay "Windows-only /
  not yet measured". Do not mark anything verified without
  evidence on a real Windows PC / real board.
- Builds, tests and releases stay local until a project is
  complete; push to GitHub only when a complete build is done.
  Never send build zips in chat — builds go to GitHub Releases.
- Signing keys live outside the repo (never committed).

## License

**MIT** — see [LICENSE](LICENSE). Third-party licenses:
[THIRD-PARTY-LICENSES.md](THIRD-PARTY-LICENSES.md).
