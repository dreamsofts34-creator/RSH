# RSH — Real Sim Hub

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)
[![Version: v1.0.0](https://img.shields.io/badge/version-v1.0.0-6C4CF1.svg)](https://github.com/dreamsofts34-creator/RSH/releases)
[![Platform: Windows 10 / 11](https://img.shields.io/badge/platform-Windows%2010%20%7C%2011-blue.svg)](docs/USER-GUIDE.md)
[![Firmware: v4.0.1](https://img.shields.io/badge/firmware-v4.0.1-green.svg)](docs/FIRMWARE-COMPAT.md)

**RSH (Real Sim Hub)** connects an ESP32 sim-racing wheel to Windows racing
games over Wi-Fi or USB-serial. The PC app is a **Tauri 2 (Rust + web
frontend)** application; the wheel firmware is **C++ v4.0.1** and the wire
protocol is **v2.1 (frozen)**.

> **This is a full Rust recreation, fresh start — not a port of the C# app.**
> The C# series (v2.0.4 and earlier) is superseded. The Rust app talks to
> the **same** C++ v4.0.1 board — if your wheel already runs firmware
> v4.0.1, **no reflash is needed** to use the Rust app. What you *do* need
> is a **fresh pairing**: the Rust app stores its device key in Windows
> Credential Manager and cannot read the C# app's DPAPI key, so every
> C# → Rust upgrader must re-pair (see [USER-GUIDE](docs/USER-GUIDE.md#re-pairing-after-the-c-rust-switch-mandatory)).

## Features

- **200 Hz wheel engine** — steering, pedals (accel / brake / clutch),
  handbrake, shifter, buttons, encoders, horn, LED strip, all processed
  at 200 Hz with per-module tuning.
- **Wi-Fi (UDP) + USB-serial** transports, HMAC-authenticated control
  channel (protocol v2.1), 60-second pairing window, per-device key.
- **Output backends** — HIDMaestro (default, user-mode UMDF2 driver via
  an elevated sidecar; UAC prompt on every launch while active) with
  **vJoy fallback** (Rust FFI, no UAC prompt). No force feedback — the
  wheel has no FFB motor.
- **Per-game + per-car profiles** — automatic switching from Assetto
  Corsa / ACC / iRacing telemetry (read-only shared memory), game
  launcher (Steam + iRacing), 3-generation profile backup.
- **8 screens** — Dashboard, Setup, Tuning, Buttons, Output, Profiles,
  Monitor, Settings — light/dark themes, English/Urdu/Hindi UI.
- **Signed firmware only** — Ed25519-signed OTA images, verify-before-
  apply, anti-downgrade. The app itself ships **unsigned** (OV code-
  signing certificate deferred — expect a Windows SmartScreen warning;
  see [USER-GUIDE](docs/USER-GUIDE.md#unsigned-app-smartscreen)).

## Quickstart

### Install (users)

1. Download **`RSH_1.0.0_x64-setup.exe`** (primary) or
   **`RSH_1.0.0_x64_en-US.msi`** (secondary) from the
   [v1.0.0 GitHub Release](https://github.com/dreamsofts34-creator/RSH/releases/tag/v1.0.0).
2. Verify the SHA-256 against `SHA256SUMS` in the same release before
   running it.
3. Run the installer — no admin prompt at install time (current-user
   install). SmartScreen will warn because the app is unsigned; choose
   "Run anyway".
4. Launch RSH, run the **Setup wizard**: connect to the wheel, pair it
   (60-second window), calibrate, set your pins.
5. Pick your output backend on the **Output** page (HIDMaestro default;
   vJoy fallback if you prefer no UAC prompt), then launch a game from
   the **Dashboard**.

Full steps, including first-run pairing and troubleshooting, are in the
[User Guide](docs/USER-GUIDE.md). **No .NET runtime is needed** — the app
is a native Tauri binary. The HIDMaestro sidecar ships inside the
installer; **vJoy is not bundled** — install it yourself only if you
want the vJoy fallback.

### Firmware — do you need to reflash?

- **Wheel already on firmware v4.0.1** (the C# v2.0.4 release shipped
  it): **no reflash**. The Rust app speaks protocol v2.1 to the same
  board.
- **New / blank ESP32 DevKit V1 board, or a board that will not boot**:
  flash the merged factory image `RSH-Firmware-DevKitV1-v4.0.1.bin`
  (955,984 B) — see the [Flash Guide](docs/FLASH-GUIDE.md) (user
  procedure) and the detailed [Reflash Guide](docs/REFLASH-GUIDE.md).
- Firmware images are **signed only** — unsigned or downgrade images
  are rejected by the board.

## Architecture (summary)

```
ESP32 wheel (C++ firmware v4.0.1, protocol v2.1)
        │  Wi-Fi UDP 4210/4211/4212  or  USB-serial
        ▼
┌─────────────────────────────────────────────────┐
│  RSH app v1.0.0 — Tauri 2 (Rust core + web UI)  │
│  crates/rsh-transport  UDP + serial + pairing   │
│  crates/rsh-engine     200 Hz WheelEngine       │
│  crates/rsh-security   keystore, HMAC, Ed25519  │
│  crates/rsh-profiles   Profile v2, 3-gen backup │
│  crates/rsh-telemetry  AC / ACC / iRacing       │
│  crates/rsh-output     HIDMaestro + vJoy        │
│  frontend/             React + Tailwind UI       │
└─────────────────────────────────────────────────┘
        │  HIDMaestro sidecar (elevated) or vJoy FFI
        ▼
   Windows DirectInput device → racing game
```

- **OS floor:** Windows 10 22H2 (build 19045) and Windows 11, x64 only.
  ARM64 is unsupported.
- **Data directory:** `%AppData%\RSH` (profiles, settings, logs
  `logs\rsh-YYYYMMDD.log`). Secrets live only in Windows Credential
  Manager (service `RSH`), never in files or logs.
- **Updater:** check-only in v1.0.0 — the app can check for updates
  against `latest.json` but cannot download/install one. Updater
  signatures are minisign/Ed25519 (public key in
  `src-tauri/tauri.conf.json`).
- Full architecture: [docs/ARCHITECTURE](docs/README.md#architecture-design)
  (index) and the plan tree in `rust-recreation/` (outside this repo).

## Repo tree

```
.
├── Cargo.toml / Cargo.lock     Rust workspace (Rust 1.100.0, Tauri 2.12.5)
├── crates/                     10 library crates (protocol, engine,
│                               transport, security, profiles, telemetry,
│                               output, output-hidmaestro, led, app-core)
├── src-tauri/                  Tauri 2 shell (rsh-app v1.0.0, NSIS + MSI)
├── frontend/                   React 19 + Vite + Tailwind v4 UI
├── tools/                      rsh-rig (Virtual Test Rig), rsh-benchmarks,
│                               rsh-hm-sidecar (C# NativeAOT, source)
├── docs/                       user + developer docs (see docs/README.md)
├── sbom/                       CycloneDX SBOMs (rust / frontend / sidecar)
├── .github/workflows/          ci.yml, release.yml (v1.* tags)
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

cd frontend && npm ci && npm test && npm run build   # vitest + tsc + vite

# Windows only (needs WebView2 / Windows SDK):
cargo tauri dev                 # run the app
cargo tauri build               # NSIS + MSI (release.yml does this on CI)
```

The Tauri shell (`src-tauri`) is a workspace member but only compiles
where the system webview libraries exist — on this Linux sandbox it does
not (no webkit2gtk dev libraries), which is a Windows-only verification
gap, not a code defect. `cargo build` / `cargo test` default to the ten
library crates on Linux.

## Documentation

| Doc | What it is |
|---|---|
| [docs/README.md](docs/README.md) | Index of every doc in `docs/` |
| [docs/USER-GUIDE.md](docs/USER-GUIDE.md) | Install, pairing, tuning, profiles, launcher, troubleshooting |
| [docs/FLASH-GUIDE.md](docs/FLASH-GUIDE.md) | Flashing firmware v4.0.1 (new board / recovery) |
| [docs/REFLASH-GUIDE.md](docs/REFLASH-GUIDE.md) | Detailed reflash + signing reference |
| [docs/GAME-MATRIX.md](docs/GAME-MATRIX.md) | Game compatibility — **0 of 16 backend–game pairs verified in-game** (Windows-only, pending) |
| [docs/RELEASE-CHECKLIST.md](docs/RELEASE-CHECKLIST.md) | Final release checklist + open gates |
| [docs/SUPPORT-PERIOD.md](docs/SUPPORT-PERIOD.md) | Support window for v1.0.0 |
| [docs/DECISIONS.md](docs/DECISIONS.md) | Frozen decisions, incl. the 4-hour soak SKIPPED by user order |
| [docs/FIRMWARE-COMPAT.md](docs/FIRMWARE-COMPAT.md) | App v1.0.0 × firmware v4.0.1 × protocol v2.1 compat |
| [docs/THREAT-MODEL-RUST.md](docs/THREAT-MODEL-RUST.md) | STRIDE threat model (Rust series) |
| [docs/SECURITY-RESCAN-H1.md](docs/SECURITY-RESCAN-H1.md) | Phase H1 security re-scan: CONDITIONAL PASS, 0 Critical / 0 High |
| [THIRD-PARTY-LICENSES.md](THIRD-PARTY-LICENSES.md) | License inventory (no GPL/AGPL/LGPL anywhere) |

## Release status (v1.0.0, 2026-11-14)

- **Phase G gate: CONDITIONAL PASS** — `cargo test --workspace
  --exclude rsh-app`: **716 passed / 0 failed**
  ([docs/PHASE-G-GATE.md](docs/PHASE-G-GATE.md)). Phase H (final) is in
  progress; see [docs/RELEASE-CHECKLIST.md](docs/RELEASE-CHECKLIST.md)
  for the open gates.
- **Known open release gates:** pid.codes PR for VID `0x1209` / PID
  `0x5253` (not yet merged); vJoy DLL SHA-256 not yet measured on
  Windows; `firmware/` must be restored in the repo before tagging;
  `RSH_UPDATER_PRIVATE_KEY` secret must be created; first
  `release.yml` dry-run not yet run; A7 real-board run pending;
  **4-hour soak SKIPPED by user decision 2026-11-14 14:54 — recorded
  as SKIPPED, never as passed** (60 s + 10 min soaks still run).
- **Game verification: 0 of 16 backend–game pairs verified in-game.**
  No game is claimed to work until the procedure in
  [docs/GAME-MATRIX.md](docs/GAME-MATRIX.md) has run on a real Windows
  PC with the real game, on that backend.

## Contributing

- Rust 1.100.0 stable (`rust-toolchain.toml`), `cargo fmt` +
  `cargo clippy -- -D warnings` clean, tests for new behaviour.
- No fake implementations: unimplemented commands stay
  `NOT_YET_IMPLEMENTED`, unmeasured claims stay "Windows-only /
  not yet measured". Do not mark anything verified without evidence
  on a real Windows PC / real board.
- Builds, tests and releases stay local until a project is complete;
  push to GitHub only when a complete build is done. Never send build
  zips in chat — builds go to GitHub Releases.
- Signing keys live outside the repo
  (`~/workspace/projects/rsh/signing-keys/`, never committed).

## License

**MIT** — see [LICENSE](LICENSE). Third-party licenses:
[THIRD-PARTY-LICENSES.md](THIRD-PARTY-LICENSES.md).
