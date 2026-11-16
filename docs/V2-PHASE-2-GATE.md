# RSH v2.0 — Phase 2 Gate: daemon + IPC + tray

**Date:** 2026-11-16
**Gate result: CONDITIONAL PASS** (two quiet-host conditions, §7)
**Commit:** see git log (`Phase 2 — daemon + IPC + tray (gate CONDITIONAL PASS)`)
**Baseline:** Phase 0 gate CONDITIONAL PASS (condition closed — quiet-host rig
27/27); Phase 1 Slint spike GO (Slint 1.18.1).

## 1. What was built

### 1.1 `crates/rsh-api` (new) — the typed daemon↔UI contract
- `AppApi` trait + `AppEvent` enum (LOGIC-FIX-PLAN §2): commands
  (`ping`, `version`, `connection`, `connect`, `disconnect`,
  `pairing_status`, `begin_pairing`, `snapshot`, `output_enabled`,
  `set_output_enabled`, `subscribe`) + push events
  (`ConnectionChanged`, `PairingWindow`, `PairingFinished`,
  `Snapshot`, `Notification`, `JobProgress`). Replaces the Tauri
  events / `drain_app_events` queue for daemon↔UI communication.
- `ConnectionManager` (LOGIC-FIX-PLAN §4.1): explicit state machine
  `Idle → LinkUp → Hello → Authenticating → Active`, with
  `Error { reason, hint, retry_in_ms }` and `Backoff { wait_ms, attempt }`.
  Constants (single-sourced here, imported by `rsh-app-core`):
  `HELLO_WAIT = 750 ms`, `HELLO_ATTEMPTS = 3`, `WATCHDOG_TIMEOUT = 2 s`,
  `BAD_TAG_RETRY_DELAY = 10 s`, `BACKOFF_BASE = 2 s`, `BACKOFF_MAX = 30 s`.
- `ConnectionError` (typed, with `code()`, `hint()`,
  `needs_user_action()`), `BindErrorKind`, `UserError::from_connection`.
- `app_version()` reads `CARGO_PKG_VERSION` (A-18).

### 1.2 `crates/rsh-ipc` (new) — local IPC, daemon host + client lib
- JSON lines, protocol version 1, envelope
  `{"v":1,"id":N,"method":...,"params":{...}}`, responses
  `{"v":1,"id":N,"ok":bool,"result"/"error":{"code","message"}}`,
  event pushes `{"v":1,"event":{...AppEvent...}}`.
- Version mismatch is answered as an error response (connection kept).
- Linux: Unix socket `<data_dir>/rsh-daemon.sock`, mode **0600**
  (verified, §6). Windows: named pipe `\\.\pipe\rsh-daemon`
  (explicit current-user DACL is an OPEN Phase-7 item, §8).
- `dispatch()` is a pure function over `Arc<dyn AppApi>`; the server
  is a thin transport around it. Client: single-in-flight-call,
  reader task demuxes responses vs event frames.

### 1.3 `crates/rsh-daemon` (new) — the always-on daemon
- Owns engine + transport + output + tray + auto profile switch.
  No web engine. Runs headless; the Slint UI (Phase 3) attaches
  over IPC.
- Threads (A-02): main thread = Slint event loop (tray only);
  `rsh-daemon-rt` tokio multi-thread runtime (2 workers) = IPC
  server + manager loop (250 ms tick); `rsh-engine-200hz` std
  thread with its own current-thread runtime (engine, owned by
  `rsh-app-core`); supervisor thread = forwarder/activator network
  runtime. No shared current-thread runtime between engine and
  network.
- Manager loop drives the `ConnectionManager` from live core
  observations (gate → `CapsOk`, typed error change →
  `ActivationFailed` + `PairingNeeded` notification, window open →
  `HelloReceived`/`Authenticating` + `PairingWindow` event),
  drains the pairing log to the diagnostics file, runs the
  heartbeat watchdog (C-04) and the backoff retry timer.
- Tray (A-09): Slint `SystemTrayIcon` (Phase 1 spike approach,
  `ui/tray.slint` copied from the spike) created on **every**
  launch, not only `--tray`. `--no-tray` disables it (headless/CI).
  Tray failure degrades to a headless park loop — the daemon keeps
  running. See §6 for the sandbox outcome.
- Single-instance: Linux pid file `<data_dir>/rsh-daemon.pid` with
  `/proc/<pid>` liveness + stale reclaim; Windows named mutex
  `Global\RSH-Daemon-<USERNAME>`. Second launch IPC-pings the
  running daemon and exits 0 (UI open hook lands in Phase 3).
- CLI: `--no-tray --no-connect --check --data-dir DIR --verbose`.
  `--check` starts, verifies IPC ping + version, exits 0/1.

### 1.4 Connection Manager fixes in `rsh-app-core` / `rsh-transport`
- **C-01/C-02:** `set_connection(Connected)` only after the session
  gate opens (signed GET_CAPS passed); `transport_state()` also
  downgrades Connected→Connecting when the gate closes. Failed
  activation enters the frame loop with the gate **closed** — the
  transport stays up, frames are held and counted (SEC-E-14), and
  re-activation is attempted when the backoff deadline fires.
  (First implementation ended the forwarder on failed activation;
  that broke the pre-existing `session_gate_holds_frames_*` test —
  fixed by the holding-loop design, not by weakening the test.)
  User-action reasons (`NoStoredKey`/`WindowClosed`/
  `StoredKeyExists`) stop auto-retry and surface "Pairing needed".
- **C-03:** HELLO wait 750 ms (> firmware 500 ms reply limiter),
  3 attempts (2250 ms total), single-sourced in `rsh-api`.
- **C-04:** heartbeat watchdog — every stream/status frame stamps
  `last_stream_frame_at`; 2 s silence in Active → transport
  rebuild + re-activate (daemon manager loop).
- **C-13:** resume fail `BadTag` → one retry after 10 s, detail
  "wheel busy".
- **C-12:** `UdpTransport::bind_typed()` returns typed
  `UdpBindError` (AddrInUse/PermissionDenied/NoInterface/Other);
  no `.ok()` drops — bind and serial-open failures are recorded
  as typed errors. Rebind waits for the old transport to drop
  (forwarder owns the transport; rebuild only after the loop ends).
- **C-11:** `udp_config_for` resolves `rsh.local` via
  `Discovery::resolve_board()` (500 ms mDNS timeout, on
  `spawn_blocking` — never on the network runtime thread),
  fallback 192.168.4.1. Address field stays IP-only in the UI
  (Phase 3).

### 1.5 Pairing logging (Phase 0 gate doc §6 — all 8 formats)
Implemented in `AppStateCore` (capped in-memory buffer, 200 lines,
drained by the daemon manager loop into the diagnostics log via
`append_startup_log_both`; every line redacted via
`diagnostics::redact_secrets` + UTC ISO timestamp from new
`diagnostics::utc_now_iso()`; one line per transition, no polling
noise):
1. `begin_pairing: transport=<wifi|usb> address=<…> attempt=1`
2. `HELO sent: session_flag=1 transport=<wifi|usb> (fresh nonce requested)`
   — on this sandbox UDP `sendto` is blocked (EPERM), so a failed
   send logs `[WARN pairing] HELO send failed: …` instead of a
   false "HELO sent" line.
3. `HELLO received: board_mac=<…> unpaired=<bool> window_open=<bool>
   key_reset_armed=<bool> password_disclosed=<bool>` (booleans only)
4. `HELLO timeout after 2250ms (attempt 3/3): board unreachable at <address> — …`
5. `window state: open (seconds_left=<N>)` / `window expired …` /
   `window closed: board already paired` (expiry logged once)
6. `activation ok: firmware=<…> protocol=<…> board_mac=<…>` /
   `activation failed: reason=<WindowClosed|NoStoredKey|BadTag|Timeout|Unreachable|…> detail=<hint>`
7. `session gate: closed -> open` / `open -> closed (reason=<…>)`
8. `one-time password disclosed to UI (single-use; consumed on first read)` /
   `one-time password consumed/dropped (window expired | paired | disconnect)`
- Typed pairing error exposed through the API
  (`AppApi::pairing_status` → `PairingInfo` with typed error;
  `AppStateCore::pairing_error_typed()`) so the Phase 3/6 UI can
  show the cause instead of one generic message.
- Regression test `begin_pairing_without_board_leaves_redacted_log_and_typed_error`
  proves a pairing attempt now leaves log lines + a typed
  `Unreachable` error; `pairing_log_redacts_planted_secrets`
  proves planted secrets never appear; `pairing_window_expiry_logs_once_not_per_poll`
  proves one line per transition.

### 1.6 Rig — pairing scenarios (Phase 0 §4.4 gap)
Three new duration-0 contract scenarios in `tools/rsh-rig`
(driving `ConnectionManager` directly; pure logic, no wheel/UDP):
- `pairing-window-open` — HELLO (unpaired, window open) →
  Authenticating → GET_CAPS ok → Active. **PASS**
- `pairing-window-expired` — HELLO (unpaired, window closed) →
  `Error(WindowClosed)`, auto-retry stopped, never Active. **PASS**
- `pairing-unreachable` — 3 HELLO timeouts → `Error(Unreachable)`,
  distinct from `WindowClosed`, never Active. **PASS**

Registry is now 30 scenarios (26 C# + `emit-200hz` + 3 pairing).

### 1.7 Version single source (A-18)
Workspace `Cargo.toml` version 1.0.2 is the one version:
`rsh_api::app_version()` = `CARGO_PKG_VERSION`; README badge
corrected v1.0.0 → v1.0.2 (was drift). `tauri.conf.json` and
`frontend/package.json` already 1.0.2. Slint UI parts (title,
Settings) come in Phase 3.

### 1.8 Parser acceptance for firmware v4.0.2 config fields (scope addition)
`crates/rsh-protocol/src/pin.rs` rejected the new v4.0.2 config
records (`unknown record type 13`). Parser-acceptance fix only —
protocol v2.1 wire format untouched:
- Record type 13 `LED_CONFIG` (`record_type::LED_CONFIG` added in
  `config.rs`): accepted, fields validated exactly like C++
  (`rsh_cfg_blob_validate`, F-14): slot 0, flags 0, no pins,
  pattern 0..4, brightness 0..100, at most one record. Maps to no
  module (the strip pin comes from the type-6 record).
- Handbrake button mode (F-15): `AXIS_ADC`/`AXIS_GENERIC` record
  with slot 4 and flags bit0 set maps to a Button module (pull-up
  rule, not the ADC1 rule); any other flag bit on a slot-4 record
  is rejected. Axis-mode records (flags 0) unchanged.
- Shifter sensitivity (D-4): `SHIFTER` `params[4]` must be 0..100
  (checked before the gate-mode check, like C++); new public
  helpers `shifter_sensitivity(blob)` (first SHIFTER record's
  field, default 50 when absent) and `shifter_debounce_ms(sens)`
  (20 − sens·15/100: 0→20, 25→17, 50→13, 75→9, 100→5).
- 15 new regression tests in `pin.rs` (LED_CONFIG valid + 6
  negatives, handbrake button valid/generic/flags/no-pullup/axis,
  shifter boundary/reject/default+debounce map). **15/15 PASS.**

### 1.9 What did NOT change
- `src-tauri/` and `frontend/` untouched — the v1.0.2 Tauri app
  still builds as-is; the daemon is additive. Phase 3 removes them.

## 2. Gate numbers

| Check | Result |
|---|---|
| `cargo test --workspace --exclude rsh-app` | **946 passed, 1 failed (environmental — see §7)** of 947 total (Phase 0 baseline 915; +17 Phase 2 new: `rsh-api` 9, `rsh-ipc` 5, `rsh-app-core` 3 pairing tests; +15 parser tests §1.8) |
| Ignored (2) | the 2 known sandbox-blocked UDP `sendto` tests (EPERM) — pre-existing |
| `cargo clippy --workspace --exclude rsh-app --all-targets` | **0 warnings** |
| `cargo fmt --check` | **clean** |
| Rig quick (30 scenarios, seed 42) | **Run 1: 18/30; run 2 (quiet attempt): 17/30 — all failures timing-only (§7)** |
| Rig pairing scenarios (affected-only + both quick runs) | **3/3 PASS** (in every run) |
| Daemon headless `--check` | **PASS** — `DAEMON CHECK OK: ping=true version=1.0.2`, exit 0; socket mode 0600 verified |
| IPC round-trip test | **PASS** (ping + version + connection + subscribe + pushed `ConnectionChanged` event; wrong-version → error response; connect-when-down fails) |
| Connection state-machine tests | **PASS** (`rsh-api` 9 tests incl. backoff 2 s→30 s, BadTag one-retry, user-action stops auto-retry, watchdog) |
| Parser regression tests (§1.8) | **15/15 PASS** |
| Tray registration (sandbox) | **Partial — see §6** |

## 3. Exit criteria check

| Criterion | Verdict | Evidence |
|---|---|---|
| Wheel reboot auto-recovers | Logic-verified | Transport loss → gate closes, records cleared, backoff rebuild + re-activate (forwarder loop); HELLO re-attempt 750 ms × 3. HIL (real board + Windows) is Phase 8. |
| Wi-Fi drop auto-recovers | Logic-verified | Same rebuild path + heartbeat watchdog (2 s silence in Active → rebuild + re-activate). HIL is Phase 8. |
| USB replug auto-recovers | Logic-verified | Serial transport factory re-opens on rebuild; open failures are typed errors, not dropped. HIL is Phase 8. |
| Tray icon on every launch | Code + sandbox-partial | Tray created unconditionally at startup (only `--no-tray` skips); §6 for the sandbox result; Windows verification is OPEN (§8). |
| Pairing attempt leaves logs | Verified | Regression test + live daemon run: HELO/timeout lines reached the log file (§6). |
| Single-instance | Verified | Second launch detected pid, IPC answered, exited 0; stale pid file reclaimed. |
| IPC current-user-only | Verified (Linux) | Socket mode 0600. Windows pipe DACL is OPEN (§8). |

## 4. Known deviations / quirks (recorded, not hidden)

- **Forwarder restructure, second iteration.** The first C-01 fix
  ended the forwarder on failed activation; that broke the
  pre-existing `session_gate_holds_frames_status_pump_and_disconnect`
  test (held frames must be counted) and its sibling (port-lock
  poisoned by the first failure). The holding-loop design (§1.4)
  keeps the transport up, holds + counts frames, and re-activates
  on the backoff deadline — the test passed unchanged afterwards.
- **HELO send on this sandbox.** UDP `sendto` is blocked (EPERM),
  so the daemon logs `[WARN pairing] HELO send failed: …` instead
  of `HELO sent`. This is the honest log line, not a bug — on a
  real network the send succeeds and the `HELO sent` line is used.
- **`transport_request` address staleness (pre-existing).** The
  `begin_pairing` log line showed `address=192.168.4.1` (captured
  at construction) while the HELLO-timeout line showed the settings
  address. Left as-is; not a Phase 2 item.
- **Daemon log location.** `append_startup_log_both` writes to the
  real data dir (`~/.local/share/RSH/logs`), not the daemon's
  `--data-dir` override — the override only moves the socket/pid
  file. Test-only quirk; production uses the real dir.
- **`cargo fmt` reformatted** `connection.rs`/`state.rs` mid-work
  (multi-line `HelloReceived` pattern etc.); the clippy fixes were
  re-applied against the formatted text.

## 5. BOOST discipline (redundant runs cut)

- During fixes: only affected tests (`-p rsh-api`, `-p rsh-ipc`,
  app-core `state::tests::begin_pairing|pairing|forwarder|activator|engine_spawn`
  subsets, rig `--only` per scenario). Full suite + rig only at the
  gate. No test round was repeated unchanged.
- Slint compile (~7 min debug build, ~5 min `cargo check`) ran
  once in the background while rig/README work proceeded.
- One full-suite run failed on 2 app-core tests; the fix was
  verified on those 2 tests only, then the full suite ran once
  more at the gate.
- The v4.0.2 parser addition (§1.8) was verified on its 15 new
  tests only during the fix; the full suite + clippy + fmt ran
  once afterwards at the gate.
- No rig re-run was looped after the two timing-failure runs:
  the failure set shuffles between runs (host contention, §7),
  so a third identical run would only burn another 12 minutes.
  The condition is recorded for a quiet-host re-run instead.
- Estimated saving vs naive loop-everything: ~2–3 h of
  repeated full-suite (~2 min each, but the rig quick is ~12 min
  and the debug daemon build ~7 min) and several rig re-runs.

## 6. Daemon headless run (this sandbox, 2026-11-16)

```
$ ./target/debug/rsh-daemon --check --no-connect --data-dir ~/tmp/rsh-daemon-final --verbose
[rsh-daemon] daemon: starting rsh-daemon v1.0.2 (data dir /home/hatch/tmp/rsh-daemon-final)
[rsh-daemon] daemon: IPC listening on /home/hatch/tmp/rsh-daemon-final/rsh-daemon.sock
DAEMON CHECK OK: ping=true version=1.0.2   (exit 0)
$ stat -c '%a' rsh-daemon.sock   → 600
```

Full run (engine + UDP connect, `--no-tray`):
- engine started, UDP transport connecting; pairing log lines
  flowed to the log (`HELO send failed` ×4 — sandbox EPERM, §4).
- Second instance: `already running (pid 21102, IPC answers: true)
  — signalling it and exiting`, exit 0. Stale pid file reclaimed
  on the next launch (exit 0).

Tray run under Xvfb (`DISPLAY=:99`, no D-Bus session bus, no tray
host): `tray.show()` returned Ok ("tray icon registered") and the
daemon stayed alive in the Slint event loop — but Slint/ksni then
reported at runtime `Failed to create system tray icon: Failed to
spawn ksni tray: D-Bus connection error`. So on this sandbox
`show() == Ok` does **not** prove an icon is visible; the failure
is environmental (no StatusNotifier host), and the daemon degrades
gracefully (keeps running, no crash). Windows tray visibility is
OPEN (§8).

## 7. Gate verdict

**CONDITIONAL PASS** (Phase 0 precedent: same shape — everything
measurable is green except two host-contention items, both to be
closed by a quiet-host re-run).

- **Condition 1 — rig quick 30/30 on a quiet host.** Run 1:
  18/30 in 733.4 s; run 2 (after a 60 s settle): 17/30 in 733.6 s.
  Every failure in both runs is timing-only, with zero logic
  failures — the failure set *shuffles* between runs (run 1 failed
  soak/emit-200hz/dropout-storm/modules-onoff; run 2 passed those
  and failed fault-wifi-drop/fault-output-drop/long-soak/ntp-ping
  instead). Typical failing assertions: `engineTicks 2106 >= 3000`
  (engine ticked ~105 Hz, not 200 Hz), `currentRatePerSecond
  148.9 >= 195`, `tickJitterP99Us 112706 <= 5000`, `validPackets
  999 > 1000`, `e2eDelayP99Us 69815 <= 10000`. In the same runs the
  functional assertions all PASS: loss 0%, coalesced 0%, CRC/auth
  clean, `engineTickJitterP99Us 1 <= 1000`. Host load during the
  runs: 2.07–4.66 on 1.5 shared CPUs, with the firmware + UI +
  docs workers building in parallel. The 3 pairing scenarios pass
  in every run. Close by: re-run `./target/release/rsh-rig --mode
  quick --out <dir>` with load average < 1.0 (no parallel builds).
- **Condition 2 — full suite 947/0 on a quiet host.** Current gate
  run: 946 passed, 1 failed — `engine_idle_without_transport_sends_no_fake_frames`
  (`loop_hz` 78–87 Hz vs the `> 150` assertion) at load 2.5–8.9.
  The same test passed in the earlier 932/0 full run in this
  session, and the value tracks host load, not code (my changes do
  not touch the engine tick path; the test has no transport). Close
  by: re-run `cargo test --workspace --exclude rsh-app` with load
  average < 1.0.
- Everything else is green: clippy 0 warnings, fmt clean, daemon
  headless + IPC + single-instance verified live, parser 15/15,
  pairing scenarios 3/3. No code change is needed to close the
  conditions — only a quiet host.

## 8. OPEN — Windows-only items (exact verification steps)

1. **Tray icon visible on Windows.** On a Windows 10/11 PC with
   the system tray: install/run `rsh-daemon` (no flags) → a tray
   icon must appear in the notification area within ~2 s of
   launch, on every launch. Right-click menu must list Open RSH /
   Send to game / Profile / Launch game / Restart wheel / Settings /
   Quit RSH. Quit must stop the daemon. (Sandbox cannot prove this:
   no tray host; `show()` Ok ≠ visible, §6.)
2. **Named-pipe current-user ACL.** On Windows, verify the pipe
   `\\.\pipe\rsh-daemon` is not connectable from another user
   account (explicit current-user DACL — currently recorded as an
   OPEN hardening item in `main.rs`; the pipe works, the DACL is
   not yet locked down).
3. **Tray popup positioning / close-on-outside-click.** Phase 1
   spike findings carry over: popup near the tray icon needs
   `Shell_NotifyIconGetRect` (Phase 7); close-on-outside-click
   unimplemented (Phase 7).
4. **Daemon install / autostart wiring.** No installer or
   autostart entry yet — Phase 7/9. Verify: after install, the
   daemon starts at logon and survives user logoff/logon.
5. **Second-instance → open UI.** Currently the second launch
   signals + exits; the UI-open hook lands in Phase 3 (Slint UI
   attaches over IPC and the daemon raises it).
6. **HIL auto-recovery.** Wheel reboot / Wi-Fi drop / USB replug
   on real hardware + Windows (Phase 8 soak): confirm Active
   returns without user action and the "Connected" label only
   shows on Active.
7. **`rsh-app` (Tauri) Windows CI.** Unchanged in this phase; the
   Tauri app is removed in Phase 3, so this is a Phase 3 item.
8. **Per-monitor DPI / tray icon scaling.** Phase 7 (Slint UI).
