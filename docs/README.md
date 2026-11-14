# RSH docs — index

**Series:** Rust app **v1.0.0** (Tauri 2, fresh start — no port of the C#
codebase) · C++ firmware **v4.0.1** · protocol **v2.1 (frozen)** ·
OS floor Windows 10 22H2 (19045) + Windows 11, x64 only.

> **Stale-doc rule:** every file in this directory is written for the
> Rust series. Where a doc mentions the C# app (v2.0.4 and earlier), it
> does so only as **historical context** (a comparison, a lesson, or a
> migration note) — never as current behaviour. There are no C#-era
> docs left here presented as current: the C# architecture / firmware /
> software / protocol / security / testing docs live only in the
> reference tree outside this repo
> (`rust-recreation/reference/docs/`), and this index does not link to
> them as current documentation.

## Start here

| Doc | Audience | What it is |
|---|---|---|
| [USER-GUIDE.md](USER-GUIDE.md) | Users | Install, first-run pairing, tuning, profiles, game launcher, output backends, troubleshooting |
| [FLASH-GUIDE.md](FLASH-GUIDE.md) | Users | Flashing firmware v4.0.1 — new board / brick recovery (user procedure) |
| [SUPPORT-PERIOD.md](SUPPORT-PERIOD.md) | Users | Support window for v1.0.0 |
| [GAME-MATRIX.md](GAME-MATRIX.md) | Users / testers | Game compatibility — **0 of 16 backend–game pairs verified in-game** (Windows-only, pending) |
| [RELEASE-CHECKLIST.md](RELEASE-CHECKLIST.md) | Release owner | Final release checklist + open gates (merges [RELEASE-CHECKLIST-H4.md](RELEASE-CHECKLIST-H4.md)) |

## Architecture & design

| Doc | What it is |
|---|---|
| [DECISIONS.md](DECISIONS.md) | Frozen spike decisions (S-4…S-13, Tauri 2.12.5 pins) + Phase E/H decisions; §13 records the **4-hour soak SKIPPED** by user order 2026-11-14 14:54 |
| [FIRMWARE-COMPAT.md](FIRMWARE-COMPAT.md) | App v1.0.0 × firmware v4.0.1 × protocol v2.1 compatibility matrix + verification evidence |
| [REFLASH-GUIDE.md](REFLASH-GUIDE.md) | Detailed reflash + firmware-signing reference (companion to [FLASH-GUIDE.md](FLASH-GUIDE.md)) |
| [KEY-LIFECYCLE-RUST.md](KEY-LIFECYCLE-RUST.md) | Key lifecycle design — per-device key, 60 s pairing window, keyring storage, C# → Rust re-pairing (§10) |
| [THREAT-MODEL-RUST.md](THREAT-MODEL-RUST.md) | STRIDE threat model for the Rust series |
| [HIDMAESTRO-SPIKE.md](HIDMAESTRO-SPIKE.md) | G1 spike report — HIDMaestro v2.1.0 GO decision (NativeAOT sidecar), UAC cost |
| [TELEMETRY-RUST.md](TELEMETRY-RUST.md) | AC / ACC / iRacing shared-memory readers (read-only) |
| [SYSTEM-INTEGRATION.md](SYSTEM-INTEGRATION.md) | Single instance, autostart, tray, Windows Firewall, update check, shutdown-neutral |
| [DIAGNOSTICS.md](DIAGNOSTICS.md) | Diagnostics export (redacted, user-consented) + crash-dump approach |
| [ANTI-CHEAT.md](ANTI-CHEAT.md) | What RSH does and does not do w.r.t. games (no injection, no memory writes) |
| [FFB-EXTENSION.md](FFB-EXTENSION.md) | **Documented only** — force feedback is NOT in v1.0.0 (no motor, no FFB code path) |

## Gates & verification

| Doc | What it is |
|---|---|
| [PHASE-F-GATE.md](PHASE-F-GATE.md) | Phase F gate — CONDITIONAL PASS (games, telemetry, profiles) |
| [PHASE-G-GATE.md](PHASE-G-GATE.md) | Phase G gate — CONDITIONAL PASS (output backends; 716 passed / 0 failed) |
| [SECURITY-RESCAN-H1.md](SECURITY-RESCAN-H1.md) | Phase H1 security re-scan — CONDITIONAL PASS, 0 Critical / 0 High |
| [RELEASE-CHECKLIST-H4.md](RELEASE-CHECKLIST-H4.md) | Phase H4 packaging evidence — NSIS + MSI, SBOMs, updater keypair, release workflow (kept as the H4 record; [RELEASE-CHECKLIST.md](RELEASE-CHECKLIST.md) is the final merged checklist) |
| [BASELINE.md](BASELINE.md) | Phase A6 performance baseline (sandbox numbers, not release gates) |

## Phase H working docs (owned by other H workers)

`PERF-H2.md` (H2 — Virtual Test Rig + soak results) and `POLISH-H3.md`
(H3 — UI polish) are owned by their workers and are linked here once
they land. The Phase H gate doc is written by the coordinator when
H1–H5 are all in.

## Outside this directory (reference only)

- `rust-recreation/` (outside the repo): master plan
  (`RSH-RUST-PLAN.md`, D-R1–D-R15), architecture, UI / firmware /
  software plans, feature inventory, research — the plans this build
  implements. `rust-recreation/reference/` holds the C#-era docs and
  the unchanged firmware v4.0.1 source; it is reference material, not
  current documentation.
- `../THIRD-PARTY-LICENSES.md`: license inventory + policy.
- `../.github/workflows/release.yml`: release workflow (v1.* tags +
  `workflow_dispatch` dry-run). Verified: YAML parses; triggers,
  secret names and the 11-asset list are recorded in
  [RELEASE-CHECKLIST.md](RELEASE-CHECKLIST.md#6-release-workflow-verification-h5-read-only).
