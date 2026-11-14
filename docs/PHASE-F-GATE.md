# Phase F Gate — Games, telemetry, profiles

**Verdict: CONDITIONAL PASS** (2026-11-14, coordinator gate round)

Phase F builds F1–F4 per the architecture + software plans (SW-R7).
All sub-phase logic is built, wired, and tested. The conditions below
are all named, owned, and carried into Phase G / H — none is a hidden
defect.

## What was built

- **F1 telemetry** (`crates/rsh-telemetry/`): `TelemetryReader` trait +
  AC / ACC / iRacing readers, `MapAccessor` seam + read-consistent
  double-read, iRacing header/var-header validation, Windows-only
  `OpenFileMappingW` + ToolHelp process gate. Fixtures: 7 synthetic
  binaries + generator. `windows = 0.62.2` spike verdict (default
  features off; ungated `windows` does NOT compile on Linux —
  reproduced; `memmap2` / `shared_memory` rejected).
- **F2 resolver / watcher / launcher**: `CarProfileResolver`
  (car-in-game → car-only → game → default → None),
  `TelemetryWatcher` (400 ms foreground debounce, car stability
  4 polls / 2 s, on-track-only switching, manual override wins,
  failed switches stay pending), `GameLauncher` (AC 244210, ACC
  805550, iRacing, Dirt Rally 2.0 690790; Steam libraryfolders.vdf
  parsing; one game at a time; stop kills only the tracked pid).
  `docs/GAME-MATRIX.md`: all 8 games, every status NOT VERIFIED.
- **F-core snapshot / profiles / commands**: `snapshot.rs`
  (`EngineMetrics` µs p99 → ms, null → "unmeasured"; `LoopRateTracker`
  sliding 1 s window; `EncoderState` wrap-aware i16 + virtual buttons
  65–72; `rsh://snapshot` payload), `store.rs` (profiles.json +
  settings.json, atomic save, 3-gen backup rotation for BOTH,
  import/export schemaVersion ≤ 2, C# manual import — Wi-Fi password
  never carried), `commands.rs` (`list_backups`,
  `get_firewall_state`, `set_output_state` / `get_output_state`,
  `get_engine_stats`, `get_snapshot`, richer `PinAssignment`
  `pins: Vec<u8>`, button bindings, tuning extensions: `bitePoint`,
  `shifterGate`, `hornButton`, `ledBrightness`, `ledPattern`).
  Fixes the E5 snapshot contract gap (µs→ms, loopHz, encoder data).
- **F-sys system integration**: `system.rs` (update check semver,
  version-aware single-instance decision, autostart stale-entry repair
  over a `RunKeyStore` trait, `TrayGuard`, firewall rule spec,
  shutdown-neutral), `diagnostics.rs` (redacted bundle export,
  panic hook), `src-tauri/src/system.rs` (Tauri/Windows glue:
  `reg.exe` Run-key store, `hide_to_tray`, Windows Firewall COM
  calls, updater `check()`, `handle_run_event`,
  `export_diagnostics_to`). Firewall spike verdict: VERIFIED (no
  netsh) — `NetFwPolicy2` / `NetFwRule` CLSIDs + `INetFwPolicy2` /
  `INetFwRule` methods verified against the `windows-0.62.2` crate
  source. Docs: `SYSTEM-INTEGRATION.md`, `DIAGNOSTICS.md`,
  `ANTI-CHEAT.md` (nothing injected, telemetry read-only shared
  memory, input only via virtual joystick).
- **Gate wiring (coordinator)**: `pub mod resolver` / `pub mod watcher`
  (`rsh-profiles`); `pub mod launcher` / `pub mod system` /
  `pub mod diagnostics` / `pub mod telemetry_adapter`
  (`rsh-app-core`, + `rsh-telemetry` dep); `TelemetryCarSource`
  adapter (F1 readers → F2 `CarSource`; `GameId::Unknown` skipped,
  menu state is not on-track); `main.rs` — `mod system`, autostart +
  updater plugins, `repair_autostart_entry()` at startup,
  `handle_run_event` → `shutdown_with_no_backend`, thin wrappers for
  `list_backups`, `get_firewall_state`, `get_autostart`,
  `set_autostart`, `check_for_update`, `export_diagnostics`;
  `tauri.ts` — 6 commands moved to `IMPLEMENTED_COMMANDS` (+
  `checkForUpdate` / `exportDiagnostics` wrappers + `UpdateInfo`
  interface).

## Verification (gate re-run)

- `cargo build` 0 errors.
- `cargo test` **475 passed / 0 failed** (21 suites:
  app-core 88, engine 79, profiles 45, protocol 60, transport 18,
  security 10, telemetry 39, + others; 3 ignored).
  Phase E was 324 — Phase F added ~151 tests.
- `cargo clippy --all-targets -- -D warnings` 0 warnings.
- `cargo fmt --check` PASS (workspace + `src-tauri`).
- Frontend: `npm test` **192/192**, `tsc --noEmit` clean.
- `cargo deny check`: advisories / bans / licenses / sources all ok.
- `cargo audit --deny warnings`: yanked check timed out again
  (registry slowness in sandbox — same as Phase E; re-run on CI /
  Phase H gate).

## Bugs found and fixed during the gate

- **G1 (spec conflict):** F2's task required "game closed → previous
  profile restored", but DATA-INPUT §3 rule 6 says a game close does
  NOT change the profile and only the launcher restores. **Decision:
  rule 6 wins — watcher default `restore_on_game_close` is OFF**
  (launcher owns restore); the behaviour stays behind the flag.
  Test `game_closed_restores_previous_profile` updated to opt in
  explicitly.
- **G2 (gate wiring):** `telemetry_adapter.rs` match delimiter dropped
  in an edit — fixed, tests green.
- **G3 (clippy):** `launcher.rs` unused `mut` + `.map().unwrap_or(0)`
  — fixed to `map_or`.
- **G4 (stale test):** `tauri.test.ts` asserted the Phase E 4-command
  set — updated to the Phase F 10-command set.
- **G5 (fmt):** `src-tauri/src/system.rs` import ordering — rustfmt
  applied.
- **Worker-fixed (carried in):** F-sys `redact_line` missed JSON
  `"api_key": "…"` + re-redaction broke the document (removed that
  pass); F-core `WheelProfile`/`AppSettings`/`PerModuleTuning`
  `Default` gave all zeros which fail the store's own validation
  (manual `Default` impls added); F2 parallel-test temp-dir flake
  (unique dir per call).

## Gate decisions (under delegation)

- **D-F1:** `get_firewall_state` wires to `commands::get_firewall_state`
  (netsh read-only probe, frontend contract `{ rulePresent, detail }`),
  NOT to F-sys's `system::firewall_rule_state` (shape
  `{ rule_present, rule_enabled, exe_path }` — a different type).
  F-sys's doc mapping was corrected at the gate. The COM path is used
  by the startup ensure path, not by this command.
- **D-F2:** watcher restore default OFF (see G1).
- **D-F3:** exit path wired via `shutdown_with_no_backend` (NullSink):
  the neutral send is attempted and honestly reported as not sent
  until Phase G wires the real backend. No fake success outcome.
- **D-F4:** stateful commands (`get_engine_stats`, `get_snapshot`,
  `set_output_state`, `get_output_state`, game commands,
  transport/pairing/config/profile commands) stay
  NOT_YET_IMPLEMENTED — their Phase F logic exists in `rsh-app-core`
  but the wrappers need `AppState`, which is the Phase G entry item.
  A stub would be a fake implementation.

## Conditions (carried, all named)

1. **AppState + stateful command wrappers** — Phase G entry item:
   engine task + metrics + `LoopRateTracker` + output backend +
   launcher in `AppState`; 30 Hz `rsh://snapshot` emission +
   `rsh://button-press` events; real shutdown-neutral actuation.
2. Tauri shell compile + window-paint smoke test — Windows-only
   (carried from Phase E).
3. First green CI run only after GitHub push — push only on the
   complete project (carried).
4. OPEN-13/14 (one credential Wi-Fi+OTA) still open (carried).
5. Windows-runtime verification open (keyring trap, serialport
   921600 + DTR no-reset, `windows` crate on Windows CI) — carried.
6. UDP sendto blocked in sandbox (rig needs a network-capable
   runner) — carried.
7. a11y (axe) audit needs a browser: Windows/manual, not run yet —
   carried.
8. `cargo audit` yanked check timed out (registry slowness) — re-run
   on CI and at the Phase H gate (carried from Phase E).
9. Live-game E2E per game; `windows` 0.62.2 on real Windows CI;
   foreground detection on Windows; ACC `acpmf_*` map-name assumption
   needs live confirmation; per-game anti-cheat spot-check open.
10. Game matrix 0/8 verified in-game on the Rust build (Windows-only).
11. Buttons 33–64 bindable decision open (rejected by
    `validate_button_bindings` — no binding UI, no C# precedent).
12. `shifterGate` tuning choice → pin-record wiring is a follow-up UI
    item (`assignment_records` always writes sequential gate).
13. Firewall rule creation/repair: COM sequence verified against crate
    source but not yet *run* on a real box (owner-PC test is the
    gate); updater E2E + minisign keypair deferred to H4
    (`pubkey: ""`, `active: false` on purpose).
14. Version-aware single-instance **takeover handover** is design-only
    (plugin callback runs in the first instance); needs a named-event
    handshake — Phase H / Windows item. Shipped behaviour is
    focus-existing; `TakeOver` means "log + focus", never a silent kill.
15. `MiniDumpWriteDump` is documented design only (`DIAGNOSTICS.md`
    §2b — needs `windows` `dbghelp` feature + owner-PC crash test).
16. Non-Windows `get_firewall_state` returns an honest "Windows-only"
    detail; `set_output_state` is a pure state transition until Phase G
    actuation lands.
