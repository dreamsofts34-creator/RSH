# RSH Rust recreation — Phase H3 polish sweep

Date: 2026-11-14. Worker: H3 (polish sweep only — H1/H2/H4/H5 are separate
workers; the gate round is the coordinator's job). Scope: architecture plan
Phase H, item H3; UI plan §5 (UI LOCK) parity checklist; Phase G gate
conditions 11, 12, 14, 15, 16 where they are UI/UX items.

Baseline at sweep start: repo HEAD 92b7b71 (Phase G), tree clean.
Frontend `npm test` 196/196; `cargo test --workspace --exclude rsh-app`
716 passed (Phase G gate). This sweep changed frontend + UI-facing Rust
only; it did not touch backends' internals, the engine, transports, or
security code.

## Verdict

**Sweep complete. 13 findings fixed with tests; 8 deferred with written
reasons; nothing left unowned.** UI LOCK parity holds except the items
listed under "Remaining known gaps" (all of them are Windows/manual-only
verification or cross-cutting engine/session work owned by other phases).

## Verification (all run 2026-11-14, Linux sandbox)

| Check | Result |
|---|---|
| Frontend `npm test` | **213 passed / 0 failed** (18 files; was 196 — +17 new assertions) |
| Frontend `npx tsc --noEmit` | clean |
| Frontend `npx eslint .` | clean |
| Frontend `npm run build` | built in 40.21 s; `index` chunk 609.89 kB (gzip 181.33 kB) — see D7 |
| `cargo test --workspace --exclude rsh-app --no-fail-fast` | **749 passed / 0 failed** (latest full run) |
| `cargo clippy --workspace --all-targets --exclude rsh-app -- -D warnings` | clean |
| `cargo fmt --check` | clean |
| `rustfmt --edition 2024 --check src-tauri/src/main.rs src-tauri/src/app_state.rs` | clean (parse only — full Tauri compile is Windows-only, see below) |

Flakes observed (not H3 regressions, all pre-existing timing tests under
the sandbox's 1.5 shared CPUs):

- `state::tests::engine_idle_without_transport_sends_no_fake_frames`
  (loop_hz measured ~76–81 Hz vs ~200 Hz expected): failed 3 times,
  passed 2 times in this sweep, including the final 125/0 rsh-app-core
  run. Timing test; needs a real machine to judge.
- `state::tests::snapshot_emitter_runs_at_30hz_without_blocking_ingest`
  (23 snapshots in ~1 s vs ~30): failed once, passed on every re-run.
- Frontend: one full run showed 3 failures in 2 files; the two
  subsequent full runs were 213/213 green. The failing tests were not
  reproduced and no touched file fails in isolation (7 touched files:
  112/112). Treated as sandbox-load flakes; the gate round should
  re-run on Windows and confirm.

## Findings

Severity: **High** = user-facing broken/misleading behaviour;
**Med** = real gap or drift, workable; **Low** = polish/note.

| ID | Sev | Screen / file | Finding | Status |
|---|---|---|---|---|
| H3-B1 | Med | Settings → System (`SystemSection.tsx`) | Stale "no backend command" copy for the firewall rule; `get_firewall_state` has existed since Phase F. | **FIXED** — wired on mount; shows present/absent + backend `detail` verbatim; loading/load-failed states. Test: `Settings.test.tsx` "firewall rule state is wired to get_firewall_state". |
| H3-B2 | High | Settings → Pins (`PinMapEditor.tsx`) | Editor was bricked when `get_pin_map` was missing: dead "unavailable" screen, even though `validate_pin_map` works. | **FIXED** — missing board map now starts from an empty draft (editor stays usable, validation runs); honest `boardMapUnavailable` notice; Apply failure via missing `set_pin_map` shows `applyUnavailable` instead of killing validation; confirm-expiry and Roll back no longer reload-and-wipe the draft when the board map is unreadable. Tests: 2 updated PinMapEditor tests. |
| H3-B3 | High | Settings → Data (`DataSection.tsx`, `lib/tauri.ts`) | `backupNow()` typed `Promise<string>` but Rust returns `Vec<BackupInfo>` — the array rendered as text; the generations list was claimed not to exist. | **FIXED** — `BackupInfo` interface added; `backupNow()`/`listBackups()` typed `Promise<BackupInfo[]>`; DataSection renders the real 3-generation list with per-generation Restore buttons (restore path = `profiles.json` / `.bak1` / `.bak2`, resolved by file name in Rust). Tests: Settings "Back up now … refreshes the generations list", "per-generation Restore …". |
| H3-B4 | Med | App shell (`AppShell.tsx`, `components/ThemeSwitch.tsx`) | Theme switch duplicated: sidebar `ThemeSwitch` (localStorage only, own state) vs Settings → Appearance (localStorage + backend settings). Sidebar change never reached backend settings; Settings then showed a stale selection after restart; sidebar never updated when Settings changed the theme. Violates UI plan golden rule 2 (theme switch ONLY in Appearance). | **FIXED** — sidebar switch removed, `ThemeSwitch.tsx` deleted (only AppShell imported it). Tests: AppShell tests assert no theme group/buttons in the sidebar. |
| H3-B5 | High | Setup → Connect (`ConnectStep.tsx`) | `run()` required a non-empty address for every transport, but the USB default address is `""` and serial auto-detects — USB connect could never be attempted ("address required"). | **FIXED** — address required only for Wi-Fi. Test: Setup "USB connect works with an empty address (the port is auto-detected)". |
| H3-B6 | Med | All screens (`lib/theme.ts`, `lib/theme.test.ts`, `index.css`) | Contrast audit (WCAG-AA ≥ 4.5:1, computed ratios). All text pairs pass in Light/Dark/High Contrast — see "Contrast audit" below. | **FIXED (verified + locked in)** — contrast pairs list extended (24 → 38 pairs: accent/accent-deep on card, muted on pill, status-text on card/pill in all three themes). Dark `--accent` as text on dark card is 3.34:1 but unused (no `text-accent`/`text-primary`/`text-info` usage in components) — documented as a non-issue in the test, not asserted. |
| H3-B7 | Med | Tuning (`TuningScreen.tsx`, `TuningPanel.tsx`) | (a) Tuning saves to the active profile but the engine still runs its default plan (Phase G condition 14) — the panel's "Saved" could be read as "applied to the wheel". (b) Extension fields (LED brightness/pattern, horn button, bite point) are session-only and have no UI controls at all in the panel. | **FIXED (a)** — honest `tuning.profileOnlyNote` under the Tuning header ("Tuning is saved to the active profile. The engine still runs its default plan, so tuning does not change the wheel output yet."). Test asserts the note. **DEFERRED (b)** — see D3. |
| H3-T1 | High | Tauri shell (`src-tauri/src/main.rs`, `app_state.rs`, `lib/tauri.ts`) | Phase G condition 15: 19 commands NOT_YET_IMPLEMENTED. 5 were UI-reachable over existing core functions. | **FIXED** — wired `get_transport_state`, `connect_transport`, `disconnect_transport`, `switch_transport`, `open_config_folder`, `get_startup_notice` (thin wrappers; `connect_transport_inner` parses kind, persists IP-literal `esp_ip`, records the request). Implemented commands 36 → 42; NOT_YET 19 → 14. Tests: `tauri.test.ts` implemented-list updated; core `transport_state`/`transport_request` tests (6). |
| H3-T2 | High | Startup (`crates/rsh-profiles/src/store.rs`, `crates/rsh-app-core/src/state.rs`, `src-tauri/src/main.rs`, `AppShell.tsx`) | Phase G condition 16: corrupt store was fail-closed (process exits, backups survive) with no repair UI. | **FIXED** — `try_repair_store_from_backup` (bak1 → bak2; restores profiles only, then profiles+settings; quarantines live files to `<name>.corrupt`); `AppStateCore::open_with_null_sink_repair` retries open after repair; repair notice stored on AppState and shown once as a dismissible banner in AppShell (`get_startup_notice`, `shell.dismissNotice`); autostart-repair failure is appended to the same notice. Tests: 4 store repair tests + 2 core open-with-repair tests + AppShell notice show/dismiss tests. |
| H3-T3 | Low | `crates/rsh-app-core/src/state.rs` `transport_state()` | Idle transport kind was hardcoded to Wi-Fi (wrong after a USB request); a failed engine showed silent `disconnected` with no error. | **FIXED** — idle kind falls back to the requested choice; engine `Failed` maps to `state = Error` + `error = engine_status_detail()` verbatim. Tests: `transport_state_idle_reports_requested_serial_kind`, `transport_state_failed_engine_reports_error_verbatim`, `transport_state_serializes_to_frontend_shape`. |
| H3-T4 | Low | `crates/rsh-app-core/src/state.rs` (doc comment) | Doc comment referenced `commands::ui_transport_state`, deleted during the H3/H1 merge of the transport-state shape. | **FIXED** — comment now references `transport_state()`. |
| H3-T5 | Low | `frontend/src/locales/{en,ur,hi}.json` | Merged global bundles still carried the 4 retired keys (`settings.data.generationsUnavailable`, `settings.data.sessionBackups`, `settings.pins.unavailable`, `settings.system.firewallUnavailable`). | **FIXED** — removed from all three global files; parity re-verified (414 keys, 0 missing/extra). |
| H3-T6 | Low | Settings → Transport (`TransportSection`, `strings.*.json`) | Address hint read "IP address for Wi-Fi, serial port for USB." — USB has no address to type (auto-detected). | **FIXED** — hint is now "IP address for Wi-Fi. The serial port is auto-detected." (en/ur/hi). |

### Contrast audit (H3-B6) — measured ratios

Light: ink/card 16.74, muted/card 6.50, muted/bg 5.93, accent-deep/card
8.03, accent/card 5.33, on-accent/accent 5.33, success-text/pill 5.78,
warning-text/card 5.43, error-text/card 6.57, error-text/danger-pill
(pass), nav-fg/nav-bg 6.78, muted/pill 5.51. Dark: ink/card 16.15,
muted/card 6.98, muted/bg 7.58, accent-deep/card 9.64, on-accent/accent
5.33, success-text/pill 6.94, warning-text/card 9.81, error-text/card
8.90, nav-fg/nav-bg 7.82, muted/pill 5.71. High contrast: ink/bg 21.00,
on-accent/accent 19.56, nav-fg/nav-bg 19.56. All ≥ 4.5:1.
Non-issue: dark `--accent` #6C4CF1 as text on dark card 3.34:1 — unused
(no component renders accent-coloured text on a dark card); status
colours as text fail on light card (info 1.81, success 1.92, warning
2.15, danger 3.27) but are only used as fills/dots/charts, never as
text — the `-text` token variants all pass.

### i18n parity (verified by script, all languages)

Global 414 keys; Dashboard 53, Tuning 47, Setup 93, Buttons 32,
Output 27, Profiles 25, Monitor 34, Settings 104 — en/ur/hi all equal,
0 missing/extra. New keys this sweep (all three languages, translated):
`settings.data.generations`, `generationCurrent`, `generationBak1`,
`generationBak2`, `savedAt`, `loadFailed`; `settings.system.firewallPresent`,
`firewallAbsent`, `firewallLoadFailed`; `settings.pins.boardMapUnavailable`,
`applyUnavailable`; `tuning.profileOnlyNote`; `shell.dismissNotice`.
Retired keys: `settings.data.generationsUnavailable`, `sessionBackups`,
`settings.system.firewallUnavailable`, `settings.pins.unavailable`
(removed from per-screen bundles and the merged global bundles).

## Deferred (with reasons)

| ID | Item | Reason |
|---|---|---|
| D1 | 14 NOT_YET commands: `begin_pairing`, `get_pairing_status`, `set_password`; `get_config`, `stage_config`, `commit_config`, `confirm_config`, `rollback_config`, `get_config_state`, `get_pin_map`, `set_pin_map`; `upload_firmware`, `get_firmware_version`, `restart_wheel` | Pairing needs the HMAC session gate (H1 owns it; wiring pairing without it would present an unpaired board as secure — Phase G condition 13). Config/pin-map need a board connection + session gate. Firmware upload/version and `restart_wheel` need a board connection. Each is listed in `tauri.ts` `NOT_YET_IMPLEMENTED_COMMANDS` and in the Tauri doc header with its reason. |
| D2 | Condition 14: per-module tunings → pipeline push | Engine-internals work (pipeline runs the default plan; clutch disabled). Cross-cutting for the gate round, not polish. The UI now says so honestly (H3-B7). |
| D3 | Tuning extension persistence (LED brightness/pattern, horn button, bite point) | `PerModuleTuning` (`crates/rsh-profiles/src/csharp_import.rs`) has no fields for them — adding fields is a profile-schema change, and the values would still not reach the pipeline (D2). Also no UI controls exist for them yet; adding controls that save session-only values the engine ignores would be fake controls. Needs D2 first, then schema + controls. |
| D4 | Condition 11: engine metrics unmeasured in snapshots (jitter/loss/E2E p99s `null`) | Engine-API work. H1 is concurrently adding per-tick metrics to `state.rs` (`ingest_engine_output`, `per_tick_metrics_reach_the_snapshot` test) — H3 did not touch it. |
| D5 | Condition 12: status packets parsed but not forwarded to engine (`btnsEx` stays 0) | Session-layer status pump. H1's in-progress `session_gate_holds_frames_status_pump_and_disconnect` test shows this work is underway — H3 did not touch it. |
| D6 | mDNS: ConnectStep implies the board is tried at `rsh.local` first, but the forwarder only dials the `esp_ip` IP literal (non-IP falls back to 192.168.4.1); `rsh.local` is never resolved | Transport/DNS work for a Phase C follow-up / gate round. `get_transport_state` honestly reports the persisted `esp_ip` instead of echoing `rsh.local`. |
| D7 | Vite build warning: `index` chunk 609.89 kB > 500 kB | Code-splitting the app is a bigger change with regression risk for a warning, not an error; app is desktop-local, not served over a network. Revisit if startup time (Windows-only measurement) shows a problem. |
| D8 | LED / horn / bite-point controls missing from TuningPanel entirely | Same as D3 — needs the pipeline push (D2) and profile fields before controls are honest. |

## UI LOCK parity checklist (UI plan §5)

| Check | Status |
|---|---|
| 8 screens, same names/order (dashboard, tuning, setup, buttons, output, profiles, monitor, settings — `lib/screens.ts` canonical order) | PASS |
| Colours match §2 tokens (violet #6C4CF1 accent, light bg #F5F3FF; `THEME_TOKENS` mirrors `index.css`, drift test enforces) | PASS |
| Dashboard: live wheel + pedals, engine stats, game launcher, Restart wheel | PASS |
| Tuning: 10 cards, one panel at a time, no toggles on cards (toggles only in Settings) | PASS |
| Setup: 7 steps in order (Connect, Pairing, Change password, Pins, Calibrate, Firmware, Finish) | PASS |
| Settings: modules / pins / appearance / data / system / transport, no repeated controls (theme switch only in Appearance after H3-B4) | PASS |
| Copy: professional English, literal text + CSS fallback (no dynamic-binding-only text) | PASS |
| No `ApexLink` strings anywhere (grep = 0) | PASS |
| No `console.log` / `alert(` in production frontend code; no TODO/FIXME/XXX | PASS |
| Every control wired — except the 14 deferred commands (D1) | PASS with deferral |
| R-P1–R-P10 performance rules: no UI-thread I/O (async Tauri invoke), 30 Hz snapshot refresh, 400 ms debounce on text/config input, LiveCharts ≤ 10 Hz redraw with 300-point ring buffer, lists capped/small (EventLog 100, profiles small — no virtualization needed) | PASS |
| Startup ≤ 1200 ms, memory flat | NOT MEASURED — Windows-only (see below) |
| Light + Dark + High Contrast themes; theme switch re-themes charts (MutationObserver) | PASS |
| i18n parity 100% (en/ur/hi) | PASS |
| a11y: keyboard-only flow (Tab order, ArrowUp/ArrowDown nav in AppShell) | PASS (code-verified) |
| a11y: axe audit, 0 critical/serious | NOT RUN — needs a browser, Windows/manual (see below) |

## Remaining known gaps

- The 14 deferred commands (D1) and conditions 11/12/14 (D2/D4/D5).
- Tuning extension fields (D3/D8); mDNS (D6); bundle chunk warning (D7).
- H1 owns the session gate and its frontend fields: the merged
  `TransportState` now also carries `session_active` and
  `frames_held_by_gate` — H3 left them unread in the UI; whether the
  frontend should surface them is a gate-round decision.
- Frontend `get_transport_state` in tests already returned the frontend
  shape; it now matches the real command.

## Windows-only verification gaps (cannot run in this Linux sandbox)

- Window-paint smoke test (Tauri shell compile + first paint on Windows).
- `src-tauri` full compile (WebKitGTK missing in sandbox; fmt-checked only).
- axe a11y audit (needs a browser, Windows/manual).
- Startup ≤ 1200 ms and flat-memory measurement on the owner PC.
- `cargo audit` re-run (timed out at the Phase G gate).
- vJoy DLL SHA-256 measured on the Windows release machine (never invented).
- pid.codes PR for `0x5253` (release gate).
- H-HID-1–H-HID-6 owner-PC checks; TCP connect-back (sandbox-blocked).
- 4-hour soak: skipped by user order (2026-11-14); only 60 s quick +
  10 min full soak run here. Release criteria must record it as
  SKIPPED by user decision, not as a pass.
- Real-board test: once, at the final production release (user rule).

## Concurrency note (for the coordinator)

H1 edited `crates/rsh-app-core/src/state.rs` live in parallel with H3.
The two `TransportState` shapes were merged by H1 into one struct
(`transport`, `state`, `address`, `error`, `session_active`,
`frames_held_by_gate`) with the frontend shape as the first four fields;
H3 converged on it and deleted its own duplicate. H1's two clippy
errors (`similar_names` in `per_tick_metrics_reach_the_snapshot`,
`large_types_passed_by_value` in `ingest_engine_output`) and the fmt
diff at `state.rs:121` were fixed by H1 during this sweep — clippy and
fmt are clean at report time. `state.rs` may still be under edit by H1;
re-verify it compiles at the gate round.
