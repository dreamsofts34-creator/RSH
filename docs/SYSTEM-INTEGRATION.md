# System Integration (SW-R7.4 / SW-R7.5 / F4)

> **Scope:** Tauri single instance, autostart, tray, Windows Firewall,
> update check, and the shutdown-neutral path. Testable logic lives in
> `crates/rsh-app-core/src/system.rs`; Tauri/Windows glue lives in
> `src-tauri/src/system.rs` (Windows CI compiles it — the Linux
> sandbox cannot).

## 1. Single instance — version-aware (C# rule)

**Shipped:** `tauri-plugin-single-instance` 2.5.5 (A2, already in
`main.rs`): a second launch focuses the existing window instead of
starting a second engine. Two instances must never fight over the
vJoy/HIDMaestro device or the UDP ports (threat TM-R08).

**The C# rule, as pure logic** (`rsh_app_core::system::resolve_instance`,
fully tested): an older version terminates, fail-closed.

| Running | This process | Decision |
|---|---|---|
| none | any | `TakeOver` — proceed as primary |
| same version | same | `FocusExisting` — focus, exit |
| newer | older | `YieldFailClosed` — this process exits |
| older | newer | `TakeOver` — the running one must exit |
| unparsable either side | any | `YieldFailClosed` — never guess |

The second instance passes its version as `--rsh-version <a.b.c>`
(`INSTANCE_VERSION_ARG`); the plugin forwards the second instance's
argv to the first instance's callback, which parses it with
`parse_instance_version_arg`.

**Gap — honest, not hidden:** the plugin cannot perform the
`TakeOver` handover. Its callback runs *in the first instance* and
the second instance exits after the callback returns, so the first
instance exiting would leave *no* instance running. A real
version-aware takeover needs a small handshake outside the plugin
(named mutex + a version file, or a named event the newer instance
waits on while the older one exits) — that is a Phase H / Windows
item, not claimed here. Until it exists, the shell ships the
plugin's behaviour (focus the existing window) and `TakeOver`
means "log the version skew and focus", never a silent kill.
Fail-closed is preserved: the ambiguous cases all yield.

## 2. Autostart — `tauri-plugin-autostart` 2.7.0

- **Enable/disable:** the plugin (`src-tauri/src/system.rs`:
  `set_autostart` / `get_autostart` via `ManagerExt::autolaunch()`).
  Init in `main.rs` (coordinator):
  `tauri_plugin_autostart::init(MacosLauncher::LaunchAgent,
  Some(vec!["--tray"]))` — autostart launches hidden to the tray
  (`AUTOSTART_ARGS`); the window only appears on tray click or a
  manual launch.
- **Registry:** on Windows the plugin writes
  `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`, value `RSH`
  (`AUTOSTART_VALUE_NAME`).
- **Stale-entry repair:** the plugin never rewrites an entry that
  points at an old folder (after a move/reinstall the Run key still
  launches a deleted exe). At startup the shell calls
  `repair_autostart_entry()` (`rsh_app_core::system::repair_autostart`
  over a `RunKeyStore`):
  - entry absent → left absent (enabling is the plugin's job);
  - entry points at the current exe (case-insensitive) → untouched,
    even with extra args (`--tray`);
  - entry points elsewhere / unquoted old path → rewritten to the
    current exe, old command reported;
  - unparseable value → rewritten (autostart was on; keep it on,
    pointed at the real exe).
  The Windows backend (`RegExeStore`) uses stock `reg.exe`
  (`query`/`add`/`delete` on the Run key) — no extra registry crate.
  `parse_reg_query_value` is unit-tested on Linux. The repair runs
  *before* the Settings page reads autostart state, so the UI never
  shows a stale "enabled" for a dead path.

## 3. Tray icon — the C# v2.0.1 trap

**Rule:** the window is hidden *only* when a tray icon actually
exists. `rsh_app_core::system::TrayGuard` authorises every hide
(`hide_to_tray` in `src-tauri/src/system.rs`); without an icon it
returns `TrayMissing` and the caller must *show* the window instead.
The icon's lifetime is owned by the shell state (`main.rs` `build_tray`
skeleton, A2): dropping the icon removes it from the tray, and a hide
after that strands the user with no way back — exactly the C# failure.
The guard's state is updated from the icon lifecycle, never from the
hide path.

## 4. Firewall — program-scoped rule (spike: VERIFIED)

**Verdict: verified path, no `netsh` (A11).** The rule is applied
through the Windows Firewall COM API, using the `windows` 0.62.2
crate (already the frozen S-10 dependency for telemetry). Verified
2026-11-14 against the `windows-0.62.2` crate source itself (the
local registry copy), not from memory:

| Item | Verified name / value |
|---|---|
| Policy coclass | `NetFwPolicy2`, CLSID `e2b3c97f-6ae1-41ac-817a-f6f92166d7dd` |
| Rule coclass | `NetFwRule`, CLSID `2c5bc43e-3369-4c33-ab0c-be9469677af4` |
| Policy interface | `INetFwPolicy2` (IID `98325047-c671-4174-8d81-defcd3f03186`), `Rules()` |
| Rules collection | `INetFwRules::Add(&rule)`, `Item(name)`, `Remove(name)` |
| Rule setters | `SetName`, `SetApplicationName`, `SetDirection`, `SetProfiles`, `SetAction`, `SetEnabled`, `SetProtocol(i32)` |
| Constants | `NET_FW_ACTION_ALLOW`, `NET_FW_PROFILE2_ALL`, `NET_FW_RULE_DIR_IN` (= 1), `NET_FW_IP_PROTOCOL_ANY` (= 256), `NET_FW_IP_PROTOCOL_TCP` (= 6), `NET_FW_IP_PROTOCOL_UDP` (= 17) |
| COM bootstrap | `CoCreateInstance(&NetFwPolicy2, None, CLSCTX_ALL)` after `CoInitializeEx(None, COINIT_APARTMENTTHREADED)` (a second init on a Tauri-owned thread is ignored) |

**Rule spec** (`FirewallRuleSpec::inbound_allow`, `FIREWALL_RULE_NAME =
"RSH"`): allow **inbound** to the RSH exe on **all profiles**, **no
port scope** — the rule is scoped to the *program*, so another
program cannot ride on the same port. Protocol `ANY` (the board
talks UDP today; a future TCP control channel stays covered by the
one rule). Outbound is untouched (Windows allows outbound by
default). `ensure_firewall_rule` (Windows-only, `src-tauri`) is
idempotent: an existing `RSH` rule is re-pointed at the current exe
instead of duplicated. `firewall_rule_state` backs the Settings
`get_firewall_state` command (present / enabled / exe path).

**Limits (not hidden):**
- Applying a rule needs **elevation**. A non-elevated apply fails
  loudly; the app keeps working without the rule and the Settings
  page tells the user to allow it (first-run firewall prompt) instead
  of silently degrading.
- The code is `#[cfg(windows)]` and **cannot compile in this
  sandbox** — Windows CI is the gate. The names above are verified
  against the crate source, but the COM call sequence (init →
  create → set → add) has not *run* anywhere yet; the owner-PC
  firewall test (SW-R7.4 acceptance) is the run gate.
- A third-party firewall managing the box can ignore the rule
  entirely; the state page reports what Windows says, not what the
  third-party product does.

## 5. Update check + Tauri updater (SW-R7.5)

**Check (implemented):** `check_for_update` (`src-tauri`) asks the
`tauri-plugin-updater` 2.14.1 plugin; `rsh_app_core::system::check_update`
compares semver (frozen `semver` 1.0.28) — newer → offer, equal →
up to date, running-newer (dev build) → never offered. The check
runs at startup in the background and never blocks the UI; a failed
check is reported as "could not check", never as "up to date".
`.NET runtime check` is gone by design (Rust has no runtime);
WebView2 ships with Windows 10/11, and a missing WebView2 is a
Tauri startup error, not something this check probes.

**Config** (`src-tauri/tauri.conf.json`, `plugins.updater`):
`active: false`, `dialog: false`, endpoints =
`https://github.com/dreamsofts34-creator/RSH/releases/latest/download/latest.json`
(the `latest.json` manifest the release workflow publishes).

**Deferred to Phase H (H4) — do NOT do now:**
- Updater **signing keypair** (`cargo tauri signer generate`,
  Ed25519, dedicated — separate from the firmware key and from the
  OV code-signing certificate; private key in the GitHub Actions
  secret, public key baked into `tauri.conf.json`
  `plugins.updater.pubkey`). `pubkey` is currently `""` and the
  updater is `active: false` on purpose: an unsigned updater must
  never offer an install. H4 fills the real public key, flips
  `active: true`, and adds the bad-signature-rejects-update test.
- Download + install (`update.download_and_install`) — check-only
  in this phase.
- No `updater:allow-*` capability is granted in
  `capabilities/default.json`: the frontend never calls the updater
  (no JS bindings are installed); the check is a Rust-side command.
  If JS bindings are ever added, the capability is added then, with
  a reason, one permission at a time.

## 6. Shutdown — restore on exit (F4)

On `RunEvent::ExitRequested` the shell runs
`rsh_app_core::system::shutdown_output` (`src-tauri/src/system.rs`:
`handle_run_event`):
1. signal the engine stop handle (the 200 Hz loop returns within
   one 5 ms tick, well inside the 5 s bound; its own `finish()`
   sends neutral again);
2. best-effort final neutral send to the output backend
   (`OutputState::neutral()` — steering centred, pedals released,
   no buttons, no gear), so the wheel never stays deflected after
   the app exits;
3. the caller then drops/releases the output backend (device
   released).

Both steps are best-effort and reported (`ShutdownOutcome`), never
panicking: a failing backend at shutdown must not turn an exit into
a crash, and a stuck engine task must never strand the exit — the
caller waits for the engine task with a timeout and exits anyway.
Sending neutral twice (shell + engine `finish()`) is harmless: the
state is idempotent.

## 7. Wiring checklist (coordinator)

- `crates/rsh-app-core/src/lib.rs`: add `pub mod system;` and
  `pub mod diagnostics;` (with doc comments).
- `src-tauri/src/main.rs`: add `mod system;`; register
  `.plugin(tauri_plugin_autostart::init(MacosLauncher::LaunchAgent,
  Some(vec!["--tray"])))` and
  `.plugin(tauri_plugin_updater::Builder::new().build())`;
  build the app and run it with `app.run(|_app, event|
  system::handle_run_event(event, || …shutdown_output…))`
  (the engine stop handle + sink come from the app state);
  add thin `#[tauri::command]` wrappers: `get_autostart` /
  `set_autostart` → `system::get_autostart` / `set_autostart`,
  `get_firewall_state` → `system::firewall_rule_state`,
  `check_for_update` → `system::check_for_update`,
  `export_diagnostics` → `system::export_diagnostics_to`;
  call `system::repair_autostart_entry()` at startup before the
  Settings page reads autostart state.
- `src-tauri/capabilities/default.json`: unchanged — no new
  frontend permission is needed for this phase (see §5).
- `Cargo.toml` (done by this worker): workspace pins
  `tauri-plugin-autostart =2.7.0`, `tauri-plugin-updater =2.14.1`,
  `windows =0.62.2` (default-features off, Windows-only use),
  `semver =1.0.28`; `src-tauri` depends on both plugins +
  `windows` under `[target.'cfg(windows)'.dependencies]`.

## 8. Windows-only gaps (everything sandbox-unverifiable)

- `src-tauri/src/system.rs` in full (Tauri + `windows` crate):
  autostart manager calls, `reg.exe` backend, COM firewall
  calls, updater `check()`, run-event shutdown wiring — Windows
  CI + owner PC are the gates. The pure logic underneath
  (`rsh-app-core`) is tested on Linux.
- Version-aware *takeover* handover (§1) — design recorded, not
  implemented; needs a named-event handshake outside the plugin.
- Firewall rule actually applied on a real box (§4) — code
  verified against the crate source, call sequence not yet run.
- Updater end-to-end (manifest fetch, signature verify, install)
  — Phase H, after the keypair exists (§5).
