# Hotfix v2.0.1 — gate note

User-PC report on v2.0.0 (2026-11-16 ~21:13 PKT): app opened, but showed
fake simulation data, dead Settings buttons, no tray icon, no setup
wizard, plus visual bugs. This hotfix fixes all five, under the user
acceptance (21:37 PKT): **everything runs on real data, nothing fake,
every button works.**

## What changed

- **Daemon actually runs:** the UI spawns the sibling `rsh-daemon.exe`
  (with retry) when no live IPC answers, and swaps to live data on
  reconnect. Installer writes the HKCU Run key for the daemon.
- **No fake data:** SimApi never feeds display values. Daemon down =
  "Disconnected" + "—" placeholders, never fake "Online". Hardcoded
  literals removed (firmware line, "4 ms / 0% / 1 ms", game states,
  profile list, backup/update claims, pedal mV defaults).
- **Every button works or is visibly disabled with a reason:** Settings,
  Pedals, Buttons, Extras, Overview, wizard and profile popover wired to
  real AppApi/IPC methods (profiles, games launch/stop, learn, tuning,
  autostart, firmware version, backup, diagnostics telemetry).
  Controls with no backend yet (centre capture, pedal min/max capture,
  button→action binding — raw mV never reaches `AppStateCore`) are
  disabled with the reason "Calibration capture needs a later wheel
  update." — v2.0.2 engine work, not faked.
- **Tray:** daemon-start chain fixed — stale `autostart=false` no longer
  deletes the installer's Run key (adopted instead), corrupt store is
  quarantined (bytes kept) instead of killing the daemon before the
  tray, tray creation retries 6×, single-instance pid-reclaim fixed,
  early deaths log to both log locations. Native menu now lists the
  real profiles + installed games.
- **Setup wizard:** first-run marker (`<data_dir>/setup-done`); opens at
  Connect (step 0), 5 steps (Connect → Drivers → Pair → Calibrate →
  Games). `PairingFinished` is now actually emitted by the daemon.
  "Run setup again" added in Settings.
- **vJoy is the default output backend** (user order 21:54 PKT —
  HIDMaestro prompts UAC on every launch). Fresh default + one-time
  migration of the persisted HIDMaestro default when vJoy is installed.
  The wizard's Drivers step checks `vjoy_status()` and installs vJoy
  itself (official BrunnerInnovation/vJoy v2.2.2.0 release, size +
  SHA-256 verified, silent install) with a HIDMaestro fallback choice
  (UAC note shown).
- **Visual/labels:** SliderRow + value boxes restyled (preview-like),
  long helper labels shortened on all screens, "daemon"/"Phase N"
  jargon out of user-facing text, pedal deadzone 10× scale bug fixed
  (slider = pct/10, handler v*10), pressed-tile fake cycle removed,
  Extras screen + profile popover wired (were dead), diagnostics drawer
  shows measured telemetry or "—" / "No telemetry yet".

## Known limitations (honest, not hidden)

- Centre / pedal min-max capture and button→action binding need engine
  work (raw mV plumbing, binding store) — v2.0.2.
- vJoy silent install + UAC-once behaviour needs a user-PC verify
  (untestable on headless Linux).
- Tray paint, Run-key login behaviour, and live board pairing need a
  user-PC/board verify.
- The tray menu is built once at daemon start; profiles created later
  in the UI appear after the next daemon start.
- Loading skeleton / rollback offer (REDESIGN-PLAN §8.6) is out of
  scope for this hotfix.
- Upgraders from v2.0.0 see the wizard once (new marker) — now useful
  because of the Drivers step.

## Gate (local only)

- Full suite (default members): **1014 passed, 0 failed** (2 ignored).
- Clippy `--all-targets`: **0 warnings**. `cargo fmt --check`: clean.
- Virtual Test Rig (`cargo run --release -p rsh-rig -- --mode quick`,
  60 s soak, within the 5-min cap): **30/30 PASS in 733.3 s**.
- No-fake audit: banned literals gone from shipped UI (one "4 ms"
  remains in a code comment only; stale Settings defaults replaced).
- Note: the old C# test-env rig (`test-env/run-virtual-rig.sh`) can no
  longer build its harness — its C# source tree was removed by the Rust
  recreation. The Rust rig (`tools/rsh-rig`) is the gate rig.
- Note: `cargo test --workspace` fails on this Linux sandbox only —
  the legacy Tauri member needs webkit2gtk (see Cargo.toml comment);
  the documented gate is the default-members run above.
