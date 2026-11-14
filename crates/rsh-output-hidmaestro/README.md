# rsh-output-hidmaestro

The HIDMaestro output backend for RSH (SW-R6.3, Phase G2). It drives
a virtual wheel through the HIDMaestro v2.1.0 SDK — via the C#
NativeAOT sidecar in `tools/rsh-hm-sidecar/` (the Rust crate never
touches the SDK). Implements `rsh_output::IJoystickOutput`, so it
is interchangeable with the vJoy backend; the selector is G4.

## Design

```
OutputState (rsh_engine) ──► mapping (this crate) ──► JSON-lines ──► rsh-hm-sidecar.exe ──► HIDMaestro SDK ──► virtual wheel
```

- **Sidecar process manager** (`manager.rs`): spawns the sidecar,
  validates its `hello` (sidecar id, protocol ≤ 1, SDK == pinned
  2.1.0, DLL SHA-256, TCP token), sends one command at a time with
  a 500 ms reply timeout, heartbeats every 1 s, detects EOF/crash.
  **Retry policy:** no silent respawn loop. The first consecutive
  failure is recoverable; the second sets `fallback_needed`
  (fail-fast `FallbackNeeded` until the user re-selects the backend
  via G4). The counter re-arms only after 200 successful submits
  (1 s of 200 Hz output) in one session.
- **Transports** (`transport.rs`): stdio JSON-lines (dev / when RSH
  itself runs elevated) and, on Windows, elevated spawn via
  `runas` + loopback TCP connect-back (handle redirection does not
  survive a `runas` launch). Same JSON framing both ways; a 128-bit
  hex token is echoed in `hello` and verified.
- **Frame mapping** (`mapping.rs`): routes through the vJoy-unit
  axis ints (`rsh_output::vjoy::steering_to_axis` /
  `pedal_to_axis`, banker's rounding) so both backends agree
  exactly. Default submit path is the 15-byte raw report (buttons
  1–32) via `use_raw_report` (M3); `SubmitState` (buttons 1–25) is
  available by opting out. Buttons 33–64 (`buttons_ex`) and encoder pulses
  65–72 have **no wire slot** on the 32-button custom profile on
  either path (same gap as the C# app).
- **Protocol** (`protocol.rs`): commands `ping` / `version` /
  `describe-profile` / `install-driver` / `create` / `submit` /
  `submit-raw` / `neutral` / `dispose` / `exit`; structured error
  codes (`not-admin`, `driver-not-installed`,
  `driver-install-failed`, `dll-hash-mismatch`, `profile-refused`,
  `create-failed`, `no-controller`, `bad-report-size`,
  `bad-command`, `unknown-command`). Full catalogue:
  `tools/rsh-hm-sidecar/PROTOCOL.md`.

## The custom profile / VID / PID

`rsh-wheel-custom`, Type `wheel`, 5 axes (X/Y/Z/Rx/Slider, 16-bit),
32 buttons, 1 hat. Descriptor: 124 bytes; derived input report:
**15 bytes** (5×16-bit axes + 32×1-bit buttons + 8-bit hat = 120
bits) — the size is derived from the descriptor by a minimal HID
parser in the sidecar, never the C# app's old 26-byte assumption
(a 128-button profile that was never built).

- **VID `0x1209`** — the pid.codes open-source pool. RSH is
  MIT-licensed, so it qualifies; the VID is shared, not a brand.
- **PID `0x5253`** — ASCII "RS". Verified absent from the pid.codes
  1209 allocation list (2026-11-14). **PROVISIONAL** until a
  pid.codes PR claiming `0x5253` for "RSH — Real Sim Hub Wheel" is
  merged — that PR is the release gate before v1.0.0. The spike's
  placeholder `0x0001` is pid.codes' own "Test PID" (PIDs
  `0x0000–0x0FFF` are reserved) and is unusable.
- No brand is spoofed: the sidecar refuses any profile id other
  than `rsh-wheel-custom`; all 50 catalog profiles are brand
  identities and are never created.

## Elevation / UAC (R-1)

The HIDMaestro path needs an elevated helper for the **whole
session** — Windows shows a UAC prompt on every RSH launch while
this backend is active (the DACL gives non-elevated processes only
GENERIC_READ on the input section). vJoy needs no runtime prompt.
`HidMaestroStatus` carries `elevation_required: true` and the
`UAC_NOTE` copy for the Output page (G4 owns the page). Do **not**
work around it with a scheduled task or a service storing
elevation.

## Pinned artifacts

HIDMaestro **v2.1.0** (latest, 2026-11-14). Release ZIP SHA-256
`7fe349f163864b8dfc36117af260d51bac94a2263b6cdfa719cb364a808397b4`;
`HIDMaestro.Core.dll` SHA-256
`f13e32591404db0f418af9265b8e240f71257297cfae5a41dcd696fd87e9b4d5`
(checked by the sidecar at startup and by the Rust `hello`
validation). The DLL is **never bundled** — the sidecar loads it
from the installed location at runtime.

## What is Windows-only

- The real sidecar + driver: install / create / submit need
  Windows + the UMDF driver (gates H-HID-1–H-HID-6, owner PC).
- NativeAOT publish: cross-OS native compilation is not supported
  (ILCompiler 10.0.15: "Cross-OS native compilation is not
  supported"). The AOT publish must happen on Windows (CI / owner
  PC). The sidecar builds and runs as a plain .NET app in the
  sandbox.
- The `windows`-crate `ShellExecuteExW` `runas` path in
  `manager.rs` (`cfg(windows)`) is not compiled in the sandbox;
  it needs a Windows CI compile check.
- The TCP connect-back path is **sandbox-blocked** in the Linux
  dev sandbox (raw TCP from a .NET process is redirected to the
  egress proxy, which refuses it). The stdio path is verified
  instead; TCP is exercised on Windows (H-HID-5).

## Tests

118 cases (target was the 78-case C# `RSH.HIDMaestro.Tests`
equivalent): lib unit 50, backend 20, manager 28, mapping 12,
protocol 8. The manager suite runs against the stub sidecar
(`src/bin/hm-stub-sidecar.rs`, same JSON protocol, env fault
injection). One test, `real_sidecar_stdio_smoke`, runs the Rust
manager against the **real production sidecar** over stdio when
`HM_REAL_SIDECAR_DLL` points at the built dll:

```sh
cargo test -p rsh-output-hidmaestro
cargo clippy -p rsh-output-hidmaestro --all-targets -- -D warnings
cargo fmt -p rsh-output-hidmaestro -- --check
```

## H-HID-1–H-HID-8 checklist (honest status)

| Gate | Status |
|---|---|
| H-HID-1 Win11 24H2, HVCI on, install/create/remove | owner-PC pending |
| H-HID-2 8-game matrix on the custom wheel profile (incl. raw-vs-`SubmitState` default) | owner-PC pending |
| H-HID-3 E2E latency + 4 h soak, IPC re-measured on Windows | owner-PC pending (Linux proxy p99 276 µs only) |
| H-HID-4 sleep/resume | owner-PC pending |
| H-HID-5 MSIX / elevated-sidecar path (`runas` + TCP) | owner-PC pending (sandbox-blocked) |
| H-HID-6 anti-cheat spot-check | owner-PC pending |
| H-HID-7 ARM64 | unsupported (stays) |
| H-HID-8 pinned version + SHA-256 in release notes + THIRD-PARTY-LICENSES.md | code pins done; release-notes entry pending |

## Known follow-ups

- pid.codes PR for PID `0x5253` (release gate before v1.0.0).
- A shared `OutputError` for G4 (the trait's error type is
  vJoy-flavoured `VJoyError` today; the mapping wart is documented
  in `error.rs` — the authoritative reason lives in
  `HidMaestroStatus` / `last_error()`).
- Windows CI compile check for the `windows`-crate `runas` path.
- Raw-vs-`SubmitState` default decision at H-HID-2.
