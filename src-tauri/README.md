# `src-tauri` — RSH Tauri 2 shell (v1.0.0)

> **DS-71 status (v2.0): KEPT-BUT-UNSHIPPED.** This shell is still a
> Cargo workspace member (`rsh-app`) and CI's `tauri-build` job still
> builds it, but the v2.0 release does NOT ship the Tauri bundle: the
> installer ships the Slint `rsh-ui` + `rsh-daemon` binaries. Its
> `tauri.conf.json` version (`1.0.2`) and updater config (`active:
> false`, no `latest.json` published) are therefore stale-by-design —
> do not "fix" them or re-enable the updater without a migration plan.
> `src-tauri/binaries/` is still live: it stages the HIDMaestro
> sidecar exe the v2.0 installer bundles beside the daemon.

Minimal app shell (Phase A2 / P2.1 scope):

- Single window (`main`), title `RSH — Real Sim Hub v1.0.0`, 1100×700.
- `tauri-plugin-single-instance`: a second launch focuses the existing window.
- Tray skeleton: `build_tray` in `src/main.rs` (wired into `setup` in P2.3,
  which also keeps the icon alive in `AppState`).
- Commands: `ping` and `get_version` only. No logic yet — later phases.
- Capabilities: default-deny, see `capabilities/README.md`.

## Verification status

- The shell is a Cargo workspace member, pinned to **tauri 2.12.5** (latest
  stable 2.x, verified against crates.io 2026-11-14) and
  **tauri-plugin-single-instance 2.5.5**.
- It cannot compile on this Linux sandbox (no webkit2gtk dev libraries) —
  Windows-only verification gap, recorded in the A2 report. On Windows 10
  22H2 / Windows 11 it builds with `cargo tauri dev` / `cargo tauri build`
  and must paint a window showing the literal text `RSH v1.0.0` (render
  smoke test is a Phase E gate).
