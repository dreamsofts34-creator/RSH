# RSH v2.0 — Phase 0 Gate: baseline + repro

- Date: 2026-11-16 (PKT)
- HEAD: `84c5f11` — "Hotfix v1.0.2: fix dialog plugin config ({} -> null) + smoke test FATAL fail-fast"
- App: v1.0.2 (Tauri 2 + React 19) · Firmware: v4.0.1 · Protocol: v2.1 (FROZEN)
- Toolchain: cargo/rustc 1.100.0, node v24.22.1, npm 10.9.4
- **Gate verdict: CONDITIONAL PASS** — Rust tests, frontend tests, clippy and
  fmt are all green, all seven [S] findings confirmed, pairing repro done,
  Slint licence verified. One condition: the rig ran **13/27** in quick mode
  (§1.1) — all 14 failures are timing/throughput assertions under a loaded
  shared host, no code changed since the earlier 27/27 gate; a re-run on a
  quiet host is required before v2.0 treats the rig as green. No code was
  changed in this phase; this doc is the only new file.

## 1. Baseline (local gate, Linux sandbox)

| Check | Result |
|---|---|
| `cargo test --workspace --exclude rsh-app` | **915 passed, 0 failed, 2 ignored, 35 suites, exit 0** |
| `cargo clippy --workspace --exclude rsh-app --all-targets` | **0 warnings, 0 errors, exit 0** |
| `cargo fmt --check` | clean, exit 0 |
| Frontend `npm test` (vitest) | **241 passed, 0 failed, 21 files, exit 0** (34.9 s) |
| Frontend `npm run build` (`tsc -b && vite build`) | OK, exit 0, built in 6.23 s; main chunk `index-BkTDpAAg.js` 653.56 kB (gzip 192.66 kB); `frontend/dist` 740 kB total |
| `cargo check -p rsh-app` (Tauri shell) | **FAILS on this sandbox — known limit, not a code bug** (see below) |
| Virtual Test Rig (`rsh-rig --mode quick`, seed 42) | **13/27 PASS in 735.1 s wall, exit 1 — CONDITION, see §1.1** |

Ignored tests (2, both in `rsh-transport`, suite 126 passed / 2 ignored):
`udp::tests::control_send_and_reply_round_trip` and
`udp::tests::helo_received_by_board_at_one_hertz` — ignored because the
sandbox blocks UDP `sendto` (EPERM); they must run where UDP send works
(Windows CI / real machine).

### 1.1 Rig (quick mode) — 13/27 PASS, CONDITION

Run: `./target/release/rsh-rig --mode quick --out ~/tmp/phase0-rig-run`
(prebuilt rig binary from 2026-11-14, seed 42). Result: **13/27 PASS in
735.1 s wall, exit 1.** Full log: `~/tmp/phase0-rig-quick.log`;
`results.json` + `REPORT.md`: `~/tmp/phase0-rig-run/`.

FAILED (14): safe-mode, modules-onoff, fuzz, chaos-net, user-chaos,
fault-reboot, fault-wifi-drop, fault-output-drop, fault-app-restart,
long-soak, ntp-ping, encoder-glitch, clutch-range, encoder-glitch-v2.

PASSED (13): soak, emit-200hz, dropout-storm, steering-calibration,
calibration-span, config-transaction, auth-gate-blocked, control-channel,
game-restore, pin-map-push, clutch-sweep, shifter-sequential,
shifter-hpattern. (Soak itself: loss 0.0%, 196.7 pps, heap slope
0.02 MB/10 min — clean.)

**Failure signature — all timing/throughput, no logic failures.**
Every failed assertion is a rate/count/latency bound, e.g.
`engineTicks: 1844 >= 3000 (FAIL)` (fuzz), `2713 >= 4500` (chaos-net),
`1894 >= 3000` (user-chaos), `currentRatePerSecond: 89.35 >= 195` (long-soak),
`silenceMaxMs: 4533 <= 2500` (fault-reboot), `e2eDelayP99Us: 69724 <= 10000`
(ntp-ping), `watchdogActivations: 0 >= 1` (safe-mode — the watchdog never
got a scheduling slice during the silence). No failed assertion is about
a wrong value, a bad CRC/auth decision, a wrong gear, or a wrong
calibration — the engine does the right things, just fewer/faster-late
than the bounds under this run.

**Why this is most likely host load, not a code regression:**
(1) No code changed since the earlier 27/27 gate — HEAD is still `84c5f11`
and the rig binary is the same Nov-14 build (`git status` clean except
this doc). (2) Load average on this shared-CPU VM was **3.41** (1-min)
during the run, on 1.5 shared CPUs. (3) The clean scenarios that need no
extra work still hit full rate (`engineTicks: 4045/4046 >= 3000` in
emit-200hz / dropout-storm) while the fault/chaos scenarios that need
extra per-packet work fall short — the classic contention pattern.
(4) Re-runs of two failing scenarios in isolation fail identically
(`--only fuzz`: `engineTicks 1932 >= 3000 FAIL`; `--only safe-mode`:
`watchdogActivations 0 >= 1, validPackets 999 > 1000 FAIL`) — repeatable
under this host, not a one-off flake.

**Required follow-up (not done here): re-run `rsh-rig --mode quick` (or
full) on a quiet host / at a quiet hour and confirm 27/27 before treating
the rig as green for v2.0.** If it still fails on a quiet host, it is a
real regression and Phase 1 must investigate before building on it. Do
not weaken any bound to pass under load — the rig's own docs say the same.

### 1.2 Known sandbox limit: `rsh-app` does not compile on Linux

`cargo check -p rsh-app` fails in `gdk-sys` pkg-config:

> `Package gdk-3.0 was not found in the pkg-config search path… The system
> library 'gdk-3.0' required by crate 'gdk-sys' was not found.`

No webkit2gtk / gdk dev libraries exist on this sandbox. This is the
documented limit in the workspace `Cargo.toml` (rsh-app is a workspace
member but excluded from `default-members` for exactly this reason). NOT
attempted to fix — Windows CI is the place this crate is verified.

### 1.3 Sizes

| Artifact | Size |
|---|---|
| v1.0.2 NSIS installer (release record, `RSH_1.0.2_x64-setup.exe`) | 11,516,382 B |
| `target/release/rsh-rig` (prebuilt 2026-11-14) | 1,853,928 B |
| `target/release/rsh-soak` (prebuilt 2026-11-14) | 532,400 B |
| `frontend/dist` (fresh `npm run build` this phase) | 740 kB |
| v4.0.1 signed firmware bin (release record) | 890,564 B; merged image 956,032 B |

No `rsh-app` release binary exists on this sandbox (cannot build here —
§1.2). Idle RAM/CPU of the v1.0.2 Tauri app **cannot be measured here**
(no GUI) — OPEN, Windows steps in §7.

## 2. Tray bug — verified from code

**CONFIRMED: the tray icon is missing on a normal launch.**

`src-tauri/src/main.rs:1141-1166` (`setup_shell`): `build_tray()` is called
**only** inside `if std::env::args().any(|arg| arg == "--tray")`. A normal
double-click launch never passes `--tray`, so `build_tray` never runs and
no tray icon exists. The `--tray` path is only reached from autostart
(`system::AUTOSTART_ARGS`, `main.rs:1224` calls `setup_shell`).

Supporting evidence:

- `main.rs:965-991` (`build_tray`): builds the `TrayIcon` with id `"main"`,
  left-click unminimize/show/focus. The icon is `!Send` and is leaked for
  the process lifetime (`std::mem::forget(icon)`, `main.rs:1151`) — but
  only on the `--tray` path.
- `main.rs:1133-1140` comment: "Bug 5 (v1.0.2): `--tray` … used to be parsed
  by nobody — autostart launches showed the window anyway and `build_tray`
  was dead code." The v1.0.2 fix wired autostart only; normal launch was
  never given a tray icon.
- There is no other `build_tray` call site in the repo (single call site,
  `main.rs:1143`).

Fix direction for Phase 2 (not done here): build the tray on every launch,
keep the window visible on a normal launch, hide it only on `--tray`.

## 3. LOGIC-FIX-PLAN Appendix A — [S] findings, confirmed / rejected

| ID | Finding (plan's claim) | Verdict | Evidence |
|---|---|---|---|
| C-10 | Firewall: app only *checks* the rule, never creates one | **CONFIRMED** | `crates/rsh-app-core/src/commands.rs:1293-1346` — `get_firewall_state()` is read-only `netsh advfirewall firewall show rule name=RSH` (Windows) / "not available on this platform" (non-Windows). No rule-creation call exists anywhere: grep for `add rule` / `advfirewall … add` / `New-NetFirewallRule` / `INetFw` in `crates/` + `src-tauri/` finds only the `FirewallRuleSpec` struct (`crates/rsh-app-core/src/system.rs:421-470`, a *spec* — `inbound_allow()` + `describe()` — with tests that only assert the description text, `system.rs:817-830`) and a doc comment. **Stale-doc note:** the `FirewallRuleSpec` doc comment claims "The rule is applied by the Windows COM code in `src-tauri/src/system.rs` (`INetFwPolicy2` / `INetFwRule`…)" — `src-tauri/src/system.rs` contains **no** firewall code at all (only a doc line at :40 pointing at `get_firewall_state`). The COM apply code does not exist; Phase 7 must write it (or correct the doc). |
| A-11 | Launcher: game spawns from RSH's working directory, not the game's folder; Stop is a hard `TerminateProcess` | **CONFIRMED** (both halves) | `crates/rsh-app-core/src/launcher.rs:562-570` — `SystemSpawner::spawn` is `std::process::Command::new(exe).spawn()` with **no `.current_dir(...)`** — the child inherits RSH's working directory. `launcher.rs:599-615` — `terminate` opens the process with `PROCESS_TERMINATE` and calls `win32::TerminateProcess(handle, 0)` directly; there is no `WM_CLOSE` / graceful-close attempt and no timeout before kill anywhere in the file. (The launcher doc header, `launcher.rs:16-17`, itself says "Stop kills only the tracked pid" — the kill is by design, the graceful step is what is missing.) |
| A-12 | UAC prompt on every launch (HIDMaestro sidecar) | **CONFIRMED** | `crates/rsh-output-hidmaestro/src/backend.rs:31-35` — `UAC_NOTE`: "HIDMaestro runs its helper elevated for the whole session — Windows will ask (UAC) on every launch while this backend is active. vJoy needs no prompt." `backend.rs:112-115, 131-132, 230-235, 308-309, 348-349` — `elevation_required: true` / `requires_elevation: true` on every backend status path. The sidecar is elevated for the whole session, not lazily on demand. (Autostart therefore means a UAC prompt at every Windows login while HIDMaestro is the active backend.) |
| C-11 | `discovery.rs` unused (mDNS `rsh.local` never resolved) | **CONFIRMED** | `crates/rsh-transport/src/discovery.rs:53-93` — `Discovery::resolve_board()` (mDNS `rsh.local`, `MDNS_HOSTNAME` at :26, fallback IP) exists and `crates/rsh-transport/src/lib.rs:15` declares `pub mod discovery`, but **no caller exists**: grep for `discovery::` / `rsh_transport::discovery` / `transport::discovery` across `crates/`, `src-tauri/`, `tools/` returns nothing outside `discovery.rs` itself (plus a doc comment in `lib.rs:4`). Frontend side: `frontend/src/screens/Setup/ConnectStep.tsx:28` hard-codes `wifi: "rsh.local"` as the address string and `frontend/src/lib/tauri.ts:36` notes "`rsh.local` is never persisted as an IP" — the literal hostname is passed to the transport as an address, never mDNS-resolved. |
| F-08 | WebOTA lockout is global (5 fails → 60 s lockout for everyone) | **CONFIRMED** | `firmware/webota.cpp:60-65, 68-85` — a **single** pair of globals, `static uint8_t authFails` and `static uint32_t authLockUntilMs`, shared by all clients; `authed()` increments the shared counter on any failed attempt and locks *everyone* out for 60 s (`kAuthMaxFails = 5`, `kAuthLockoutMs = 60000`). No client IP is recorded or compared anywhere in the file — there is no per-IP state to make it per-IP. Any station on the AP can lock out a legitimate upload with 5 bad passwords. |
| F-09 | Secrets at rest: device key plaintext in NVS, password only XOR-obfuscated; flash/NVS encryption only in the secure build | **CONFIRMED** | `firmware/keystore.cpp:140` — `prefs.putBytes("devkey", s_key, sizeof(s_key))` stores the device key **plaintext** in NVS. `firmware/pw_obfuscate.h:2-9` — the password is XORed with a keystream derived from that same `s_key` ("obfuscation", explicitly not encryption; the file header says so). `firmware/platformio.ini:16-37` — the **default** env `[env:esp32dev]` has no `RSH_SECURE_NVS` flag and uses `partitions = default.csv`; the secure env `[env:esp32dev-secure]` (`platformio.ini:67-86`) adds `-DRSH_SECURE_NVS=1` and `partitions-secure.csv`, but its own comment says it was **never build-verified in this sandbox** and that plain-Arduino precompiled IDF does not even apply the sdkconfig encryption flags (IDF source build required). The v4.0.1 release image is the default-env build. |
| F-13 | Single-PC peer model (`havePc`) | **CONFIRMED** | `firmware/net.cpp:25` — `static bool havePc = false;` single peer slot. `net.cpp:480-489` — HELO acceptance keys off `havePc`/same-IP; `net.cpp:516` — liveness requires `havePc`; `net.cpp:766` — stream loop `if (havePc && !(sender == pcIp)) continue; // sirf HELO wala peer` — packets from any second PC/phone are dropped. By design per the plan; needs documenting, not a code fix. |

All seven [S] findings are **CONFIRMED**; none rejected. (C-11 is tagged
[V] in the plan's Appendix A but was listed for re-confirmation in this
phase's brief — it confirms the same way.)

## 4. Pairing repro — the live bug

Live symptom (user's Win10 22H2 PC): RSH Wi-Fi appears OPEN and connects,
but Begin pairing always reports "No pairing window is open", even on fast
reboot attempts. v1.0.2 startup log contains no pairing/HELO/HELLO lines.

### 4.1 (a) Repro: Begin pairing after the 60 s window → "No pairing window"

**Reproduced at unit level** (the rig cannot do this — §4.4). Existing test
`state::tests::begin_pairing_without_board_fails_closed_and_reports`
(`crates/rsh-app-core/src/state.rs:7755-7792`), run this phase:

```
cargo test -p rsh-app-core --lib \
  state::tests::begin_pairing_without_board_fails_closed_and_reports
→ test result: ok. 1 passed; 0 failed (1.09 s)
```

With no board answering, `begin_pairing()` returns `PairingStatus`
`{ seconds_left: 0, password: None }`, the session gate stays closed, the
activation failure is recorded (`session_last_error()` becomes `Some`
within 10 s), and `session_activation()` stays `None`. That is exactly the
user-visible outcome: the UI shows `setup.pairing.noWindow` —
"No pairing window is open." (`frontend/src/screens/Setup/strings.en.json`,
`pairing.noWindow`).

Firmware side (why the window is already gone by the time the user clicks):
the 60 s window runs from **AP-up, not from the user's click** —
`firmware/keystore.cpp:195-197` (`keystore_pairing_open`: `!s_apUp → false`,
`(now - s_apUpMs) < 60000`) and `firmware/net.cpp:274-280` (window marked
once, `apUpMarked` guard: "rescan par AP restart ho to window dobara NAHI
khulti"). HELLO discloses credentials only while the window is open —
`firmware/net.cpp:318, 328` (`keystore_pairing_open(millis()) &&
keystore_key()` → `RSH_V2_HELLO_WINDOW_OPEN` flag, else zero key/password).
So a board that has been on for > 60 s (the normal case: wheel on, Windows
joins Wi-Fi, app opens, wizard runs) has **no window at all**, and no
in-app action reopens it (pair button does not exist in v4.0.1 — F-02).
The user's report — open Wi-Fi connects fine, pairing always fails, even
on fast reboot attempts — matches this exactly: connecting to the open AP
proves the radio/link, but the window had already expired before
`begin_pairing` ran. The "fast reboot" attempts fail the same way because
the 60 s budget is spent on boot + Windows Wi-Fi join + app start before
the click.

Also relevant: the app-side window clock is `observed_at` of the last
HELLO (`crates/rsh-app-core/src/state.rs:551, 1156-1159`,
`PAIRING_WINDOW_SECS = 60`, `crates/rsh-protocol/src/hello.rs:20`) — the
app never learns the board's remaining window time, it assumes 60 s from
first sight of a HELLO (C-05).

### 4.2 (b) Pairing path writes no log lines — CONFIRMED (diagnostic gap)

`grep -c "log::" crates/rsh-app-core/src/state.rs` → **0**. The entire
8,000+-line state machine — including `begin_pairing`
(`state.rs:1252-1273`), `activate_session`, `note_observed_hello`
(`state.rs:1071-1100`), `note_session_error` (`state.rs:1060-1068`),
`pairing_status`/`pairing_status_peek` (`state.rs:1136-1190`) — contains
**no logging call at all** (no `log::*`, no `tracing::*`, no `eprintln!`).
The only log output the v1.0.2 app produces is the startup log
(`src-tauri/src/main.rs:121-132` `init_startup_logging` / `log_startup`,
dual location %APPDATA% + %TEMP%), which records data-dir resolution,
backend activation and "setup complete" — and nothing about pairing,
HELO, HELLO, window state or activation transitions. That is why the
user's `rsh-20261112.log` had only startup lines: the pairing path is
silent by construction, not because `main()` never ran. (The redaction
helper the fix should reuse already exists:
`crates/rsh-app-core/src/diagnostics.rs:85` `redact_secrets`.)

### 4.3 (c) Can the app/UI distinguish "board unreachable" from "window closed"? — NO

The backend **can** tell them apart, the UI **cannot**:

- Backend: `PairingStatus` carries both signals —
  `seconds_left` (0 = no window) and `error: Option<String>` (the last
  activation failure, `state.rs:1186`, `error: self.session_last_error()`).
  With no board at all, `begin_pairing` returns `seconds_left: 0` **and**
  a recorded error (§4.1 test asserts `session_last_error().is_some()`).
  With a reachable board whose window expired, `seconds_left` is 0 with
  no (or a different) error. The distinction exists in the data.
- UI: `frontend/src/screens/Setup/PairingStep.tsx` **never reads
  `status.error`**. It reads only `status.password` and
  `status.secondsLeft` (`PairingStep.tsx:59-61, 74-76, 87`). The local
  `error` state is set only from *invoke rejections* (`handleBegin` /
  `handleRefresh` `.catch(...)` → `describeError`), never from the
  `PairingStatus.error` field. When there is no password and the step is
  not acknowledged, the UI shows one generic notice for every cause
  (`PairingStep.tsx:155-158`):
  `{t("setup.pairing.noWindow")}` = "No pairing window is open."
  So "board unreachable", "window expired", and "board paired already"
  all render identically. The user cannot tell which one happened, and
  neither can support from a screenshot.

Fix direction for Phase 2/3: surface `status.error` in `PairingStep`
(distinct copy for unreachable vs window-closed vs already-paired), and
add the logging spec in §6 so the log also shows it.

### 4.4 Rig limitation

The Virtual Test Rig cannot reproduce this bug: its 27 scenarios (§1.1)
model the *stream* path (channel transport, no real UDP, no board, no
pairing window, no HELLO). There is no pairing/window scenario in the rig.
Pairing repro therefore lives at the `rsh-app-core` unit level (§4.1) and,
for end-to-end, on the HIL matrix (Phase 0 plan §7) — Windows + real board,
which this sandbox cannot do. Recommendation: add a rig scenario (or a
`rsh-app-core` integration test) that drives `begin_pairing` against a
fake board whose window expires mid-run, so the "window closed" vs
"unreachable" distinction is covered in automation.

## 5. Slint licence (decision D-12) — VERDICT: Royalty-free 2.0 is usable; GPLv3 is FORBIDDEN

Verified 2026-11-16 from Slint's **official** FAQ (fetched live) and the
Royalty-free 2.0 licence text.

**Verdict: RSH (MIT-licensed desktop app) may use Slint under the
Royalty-free License 2.0, free of charge, provided it shows the Slint
attribution (AboutSlint widget in an About screen/dialog reachable from
the top-level menu, or the "Made with Slint" badge on the public download
page). GPLv3 must NOT be used for RSH.**

What the official FAQ says (verbatim substance):

- "You can use Slint under *any* of the following licenses, at your
  choice: 1. Royalty-free license, 2. GNU GPLv3, 3. Commercial license."
- "Can I keep my own code under a permissive license such as MIT? **Yes —
  whichever Slint license you use, your own source files can stay under a
  permissive license such as MIT or Apache-2.0.**"
- Royalty-free obligations: display the `AboutSlint` widget in an "About"
  screen/dialog accessible from the top-level menu (or in the splash
  screen if there is no About screen), **or** display the Slint
  attribution badge on a public webpage, preferably where the binaries can
  be downloaded, easily findable.
- Royalty-free limitations: (1) may not distribute Slint standalone
  without integration into an application; (2) **may not use Slint within
  Embedded Systems**; (3) may not distribute an application that exposes
  Slint's APIs in part or in total; (4) may not remove/alter licence
  notices in Slint source.
- "Embedded system" definition: a computer system performing a specific
  task within a larger mechanical/electrical system (e.g. appliance
  controller, POS terminal, car dashboard). "**It's *not* an embedded
  system when your application runs on a user's own general-purpose
  computer or phone, installed as one application among many.** That's the
  desktop, mobile, and web case the Royalty-free License covers for
  free." — RSH's PC app is a desktop app on the user's own Windows PC, so
  it is covered. The ESP32 firmware is C++, not Slint, so the embedded
  exclusion does not touch the board side.
- GPLv3 consequences: "While your software modules can remain under the
  MIT-license, **the work as a whole must be licensed under the GPL**."
  ("Open-source the whole product? Use the GPLv3. The product as a whole
  is then GPL…") — that would re-licence the distributed RSH product as
  GPLv3, which contradicts RSH's MIT licence and the user's standing
  order. **GPLv3 is therefore forbidden for RSH.** (A fork could
  re-license only by acquiring a Slint proprietary licence or removing the
  Slint dependency — irrelevant to the shipped product.)
- Fork scenario: a fork of MIT-licensed RSH may itself choose any of the
  three Slint licences; the choice binds the fork, not RSH.

Sources (official, fetched 2026-11-16):

- https://raw.githubusercontent.com/slint-ui/slint/master/FAQ.md
  (Licensing section — the quotes above are from this file)
- https://raw.githubusercontent.com/slint-ui/slint/master/LICENSES/LicenseRef-Slint-Royalty-free-2.0.md
  (Royalty-free Desktop, Mobile, and Web Applications License, v2.0 —
  grant §1, attribution §2)
- https://slint.dev/blog/making-slint-desktop-ready (licence comparison
  table: Royalty-Free = free, any licence, desktop/mobile/web, attribution
  required; GPLv3 = distributed binary must also be GPLv3)
- https://slint.dev/pricing (free tier: "Keep your code proprietary.
  Credit Slint."; commercial covers embedded / no-attribution)

Action for Phase 1/3: record the licence choice (Royalty-free 2.0) in the
repo (e.g. `THIRD-PARTY-LICENSES.md` + a copy of the licence text), add the
`AboutSlint` widget to the About screen (the v2.0 design has no About
screen yet — one must be added, reachable from the top-level menu, or use
the badge on the GitHub release page), and never select GPLv3 in any
dependency/license metadata. This is not legal advice; if the user wants a
formal opinion, that is a lawyer's job, not this gate's.

## 6. Spec: pairing logging fix (for the Phase 2 team)

**Problem:** the pairing/activation path logs nothing (§4.2), so a failed
pairing leaves no trace. **Goal:** every pairing attempt must leave a
redacted, timestamped trail in the startup/diagnostics log that lets
support distinguish unreachable / window-closed / already-paired /
bad-key without ever exposing a secret.

### 6.1 Where

Add logging to `crates/rsh-app-core/src/state.rs` (activator + pairing
status paths) and surface the same events through the existing
`queue_app_event` / diagnostics log tail (`diagnostics.rs`). Reuse the
existing startup-log writer (`src-tauri/src/main.rs` `log_startup`) or the
diagnostics log path — one file, one ordering, no second log. Do **not**
add a new logging framework; `log` crate with the existing sink is fine
if one is introduced, otherwise the existing `log_startup`-style
file-append path. All lines go through `diagnostics::redact_secrets`
before being written.

### 6.2 Lines to emit (exact formats; `[...]` = redacted placeholder)

All lines: UTC timestamp + level + component tag, e.g.
`[2026-11-16T06:30:00Z INFO pairing] …`. Never log: device key, Wi-Fi
password, HMAC tag, nonce/counter values, or full HELLO bytes. Board MAC
may be logged (it is a local-network identifier, already shown in the UI).

1. Attempt start (in `begin_pairing`, before `activate_session`):
   `[INFO pairing] begin_pairing: transport=<wifi|usb> address=<redacted-ip-or-host> attempt=1`
2. HELO sent (activator, flag-1 HELO):
   `[INFO pairing] HELO sent: session_flag=1 transport=<wifi|usb> (fresh nonce requested)`
3. HELLO received (in `note_observed_hello`):
   `[INFO pairing] HELLO received: board_mac=<AA:BB:..> unpaired=<true|false> window_open=<true|false> key_reset_armed=<true|false> password_disclosed=<true|false>`
   (password_disclosed is a boolean only — never the password, never its
   length beyond the fixed 24-char field.)
4. HELLO timeout / no reply (activator retry expiry):
   `[WARN pairing] HELLO timeout after <N>ms (attempt <i>/3): board unreachable at <address> — check Wi-Fi is joined to the RSH network`
5. Window state transitions (in `pairing_status_peek` / `note_observed_hello`):
   `[INFO pairing] window state: open (seconds_left=<N>)` on first sight of
   an open window; `[INFO pairing] window expired (60 s from first HELLO)` when
   `seconds_left` reaches 0; `[INFO pairing] window closed: board already paired`
   when a HELLO arrives with `unpaired=false`.
6. Activation result (success / failure, in `activate_session` /
   `note_session_error`):
   `[INFO pairing] activation ok: firmware=<x.y.z> protocol=<x.y> board_mac=<…>`
   `[ERROR pairing] activation failed: reason=<typed reason, e.g. WindowClosed|NoStoredKey|BadTag|Timeout|Unreachable> detail=<one-line hint>`
7. Session gate transitions:
   `[INFO pairing] session gate: closed -> open` / `open -> closed (reason=<…>)`
8. Password lifecycle (no secret, ever):
   `[INFO pairing] one-time password disclosed to UI (single-use; consumed on first read)`
   `[INFO pairing] one-time password consumed/dropped (window expired | paired | disconnect)`

### 6.3 Rules

- **Redaction is mandatory and tested:** unit test that feeds a log line
  containing a 24-char password / 32-byte key / HMAC hex through
  `redact_secrets` and asserts the secret does not appear in the output.
  Add a test that runs `begin_pairing` with no board and asserts the log
  contains the `HELLO timeout` and `activation failed: reason=…` lines
  (this test is the regression test for this spec).
- **One line per transition, not per poll:** the 5 s UI poll
  (`PairingStep` re-poll) must NOT log; only state changes log. The
  1 s UI countdown must never log.
- **Keep the existing single-use password rule (M-9):** logging must not
  create a second copy of the password anywhere (no log, no event
  payload, no diagnostics bundle content beyond the boolean above).
- **UI pairing:** `PairingStep.tsx` must read `status.error` and show
  distinct copy for `Unreachable` ("Wheel not found — check the Wi-Fi…")
  vs `WindowClosed` ("Pairing window closed — restart the wheel…") vs
  already-paired, instead of the single "No pairing window is open."
  (§4.3). The log line and the UI copy must agree on the reason string.

## 7. OPEN items — Windows-only verification (exact steps)

These could not be measured on this Linux sandbox. Do not treat the Phase 0
PASS as covering them; each needs a real Windows run.

1. **Idle RAM/CPU of the v1.0.2 Tauri app (baseline for the Slint
   comparison).** Steps: install `RSH_1.0.2_x64-setup.exe` on Win10/11,
   launch, wait 60 s idle with wheel disconnected, then 60 s idle
   connected; record in Task Manager → Details → RSH.exe: Working Set
   (private) MB and CPU % (average over the window). Repeat with the
   window minimised. Record all four numbers.
2. **Tray icon on normal launch (visual).** Steps: double-click the
   installed app (no `--tray`); check the system tray (including the
   hidden-icons overflow) for the RSH icon; close the window and check
   whether the app keeps running in the tray. Expected now: NO icon
   (§2) — this is the before-measurement for the Phase 2 fix.
3. **Real UDP pairing end-to-end.** Steps: fresh board (unpaired), boot,
   join RSH Wi-Fi within 60 s, Begin pairing in the app; record
   seconds-left shown and whether the password appears. Then repeat with
   the board on for > 60 s before clicking Begin (expect "No pairing
   window" — §4.1). Attach the log file from `%AppData%\RSH\logs` — after
   §6 lands it must contain the §6.2 lines.
4. **Firewall rule creation (C-10 fix, Phase 7).** Steps: clean Windows
   VM, no RSH rule (`netsh advfirewall firewall show rule name=RSH` →
   no match); install; check the rule now exists and is program-scoped to
   the installed exe; verify 200 Hz stream arrives (no "firewall
   blocking" state).
5. **UAC behaviour (A-12).** Steps: HIDMaestro backend active, launch app
   from a normal (non-admin) account; count UAC prompts per launch and at
   Windows login with autostart on. Expected now: one prompt per launch.
6. **Game matrix (0/16 in v1.0.2).** AC, ACC, iRacing × HIDMaestro, vJoy
   = 6 priority pairs minimum; launch/stop via the app, confirm input
   reaches the game.
7. **rsh-app compile + clippy on Windows CI.** The Linux gate excludes
   `rsh-app` (§1.2); Windows CI must stay green on it.
8. **4-hour soak (v2.0).** Skipped for v1.0.0 by user order; the v2.0 plan
   requires it — `rsh-rig --mode long --only long-soak`, loss < 0.5%.

## 8. What Phase 1/2 need from Phase 0

- **Baseline to beat:** Rust 915/0/2, frontend 241/241, clippy 0, fmt
  clean, rig quick **13/27 (CONDITION — quiet-host re-run required,
  §1.1)**. Idle RAM/CPU baseline is still OPEN
  (§7.1) — Phase 1's spike must measure it first, on Windows, before
  claiming any Slint improvement.
- **Tray:** fix is unblocked and specified (§2) — build the tray on every
  launch, not only `--tray`.
- **Pairing:** root cause is the 60 s AP-up window (§4.1) + silent logging
  (§4.2) + an undifferentiated UI error (§4.3). The firmware fix (F-02
  pair button, 120 s window) and the app fix (Pairing Manager, §6 logging,
  `status.error` in the UI) are all specified; no further diagnosis is
  needed.
- **[S] findings:** all seven confirmed — Phase 2/5/7 work can proceed on
  them as facts (firewall rule creation, launcher cwd + graceful stop,
  UAC lazy-start, mDNS wiring, per-IP WebOTA lockout, secrets-at-rest
  decision D-11, single-PC documentation).
- **Licence:** Slint under Royalty-free 2.0 with AboutSlint attribution;
  GPLv3 forbidden (§5). Phase 1 must record the choice and add the
  About screen/badge.
- **Rig gap:** no pairing scenario exists (§4.4) — add one before relying
  on the rig for pairing regressions.

## Follow-up — quiet-host rig re-run (2026-11-16, coordinator)

- Re-run: `./target/release/rsh-rig --mode quick --out ~/tmp/phase0-rig-rerun`
  (same prebuilt Nov-14 binary, seed 42), host quiet (load 0.26 at start).
- Result: **27/27 PASS in 733.1 s, exit 0.** Loss 0.0%.
- The §1.1 CONDITION is **CLOSED**: the 13/27 was host contention
  (load avg 3.41 on 1.5 shared CPUs during the first run), not a
  regression — no code changed between the two runs. The rig baseline
  for v2.0 is green; Phase 2 gates on it.
