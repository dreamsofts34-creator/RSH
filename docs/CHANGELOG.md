# Changelog — RSH (Real Sim Hub)

All notable changes to the RSH app and firmware. Current versions:
**app v2.0.16 · firmware v4.0.6 · protocol v2.2 (frozen)**.
Release notes on GitHub are the same text, shorter; this file is the
full record. The C# series (v2.0.4 and earlier) is superseded — the
Rust series below starts at v1.0.0.

## [2.0.16] — 2026-11-27 (HIDMaestro G29 spoof mode)

App 2.0.16 + firmware 4.0.6 unchanged (protocol v2.2 frozen — no wire-format
change, no firmware code change in this release).

**G29 spoof mode (HID backend only, default OFF).** New Settings → Game
Controller toggle presents the wheel to games as a Logitech G29 (VID 046D /
PID C24F, product string "G29 Driving Force Racing Wheel") using HIDMaestro's
built-in `logitech-g29` profile — the 123-byte C24F descriptor, verified
byte-for-byte against the pinned v2.1.0 DLL (never hand-rolled). Steering → X
16-bit (centre 32768); pedals → Z/Rz/Y 8-bit (accelerator/brake/clutch per the
HIDMaestro profile layout — PCGamingWiki disagrees on which 8-bit field is the
accelerator, so the assignment is one constant away from swapping and must be
verified on the PC test); 25 buttons per the G29 map; D-pad = hat; handbrake →
L2 button past 50% (the G29 has no handbrake axis). Force-feedback writes from
the game are accepted and discarded (the wheel has no motor — no FFB, no
centring spring). Toggling the mode restarts/reconnects the HID backend with
the other profile; turning it off restores the RSH 1209/5253 identity
byte-for-byte. Enabling on the vJoy backend is refused (`g29_spoof_hid_only`);
disabling always succeeds. **Honest labelling:** this presents the wheel AS a
G29 for game compatibility; it is not a Logitech product. **Expectations
(research, not proven):** even a real G29 is "not recognized in game by
default" on FH4 and needs a custom control scheme (Forza Known Issues) —
expect to bind a custom wheel profile in FH4, not auto-recognition (~50%
usable via custom profile, ~25–35% auto-recognition). **Risks (disclosed, not
hidden):** using Logitech's VID/PID violates USB-IF policy as written (no
enforcement found for private hobby use); no Forza ToS text on device spoofing
was found (risk low, not zero); the pid.codes 0x1209/0x5253 claim is unaffected
(simply unused while G29 mode is on). The Steam FH4 (OEMData) toggle and code
stay in place as a fallback until G29 mode is proven on the PC.

**DeferredApi forwarding fix (v2.0.15 lesson).** The v2.0.15 Steam FH4 toggle
failed on the PC with "api not supported": the daemon's IPC server holds a
`DeferredApi` that never forwarded `set_steam_fh4_mode`, so the call fell
through to the trait default and the live daemon implementation was unreachable
(the gate's IPC round-trip test had exercised the daemon directly, not through
`DeferredApi`). The G29 toggle is wired through the same path, so
`g29_spoof_mode` / `set_g29_spoof_mode` are now forwarded in `DeferredApi`,
with a test that drives the exact UI call path (IPC dispatch → `DeferredApi` →
`DaemonApi`). The FH4 toggle itself is intentionally NOT fixed (user decision
2026-11-27: G29 replaces it; the OEMData code stays as fallback).

## [2.0.15] — 2026-11-27 (FH4 Steam wheel mode + mouse-click fix)

App 2.0.15 + firmware 4.0.6 unchanged (protocol v2.2 frozen — no wire-format
change, no firmware code change in this release). Two user-found fixes,
both production-ready in this batch.

**Steam FH4 wheel mode (Part A).** The user found and PC-tested it
himself: writing `OEMData` REG_BINARY `40 00 00 00 00 00 00 00` under
the vJoy OEM registry key makes the Steam version of Forza Horizon 4
show the Wheel tab (v2.0.14's complete GameInput mapping alone did not
do it for the Steam build). New Settings toggle "Steam FH4 wheel mode"
(default OFF, vJoy only — disabled on the HIDMaestro backend, which
never gets the flag: `VID_1209&PID_5253` is never written). Turning it
on writes the exact bytes to the vJoy OEM key (`VID_1234&PID_BEAD`) in
both HKCU (daemon, user context) and HKLM (one elevated run, one UAC
prompt — skipped when HKLM already holds the flag); turning it off
deletes only the `OEMData` value, never the key (it is vJoy's). The
installer has an optional "Steam FH4 wheel mode" checkbox (off by
default; silent installs never get it) that writes the HKLM flag plus
a marker the daemon adopts on first read; the uninstaller deletes the
flag only when that marker says RSH wrote it. The toggle warns to
disable/enable vJoy and restart the PC. **Store-version risk (UNVERIFIED
community claim, not tested): the flag may break the Microsoft Store
version of FH4 — this mode is Steam-only, keep it off for the Store
version.** Revert: toggle off, disable/enable vJoy, reboot (uninstaller
also removes the flag when RSH wrote it; a standard-user install's
HKCU value is removed by toggling off, since an elevated uninstaller
cannot reach that hive). **Joystick Gremlin warning: RSH sends 8 axes
and issue #471 reports a crash with 8 axes + this flag — Gremlin users
should keep the mode off.**

**Mouse 2-click fix (Part B).** After the Wheel tab appeared, FH4 menus
needed two clicks: the pointer flickered on/off within milliseconds and
the first click did nothing. Cause in code (verified): the engine feeds
`UpdateVJD` 200×/s unconditionally, and ESP32 ADC idle jitter (~±24
axis counts) meant almost every tick carried a slightly different
position — the game kept flipping between controller mode and mouse
mode. Fix, gated behind Steam FH4 wheel mode so other users' behaviour
is unchanged: identical positions are not re-sent (100 ms keepalive,
so the device still looks live at 10 Hz idle), and sub-32-count axis
changes are held at the last sent value (32 counts ≈ 0.1% of range —
above idle jitter, below any deliberate input; buttons/hats never
filtered; any over-threshold move sends all axes fresh, no lag or
precision loss). The game-side mode-flip mechanism itself is unverified
(no source) — the PC retest is the proof: one click in FH4 menus with
the mode on. If it still takes two clicks, the cause is the game's own
behaviour with any active wheel, and the workaround is keyboard/wheel
in menus.

Tests/gates: 729 unit tests pass (incl. exact flag bytes, both-hives
script, value-only disable, dedupe/keepalive/hysteresis against the
mock DLL, IPC round-trip for the new methods); packaging gate asserts
the default install writes no OEMData (checkbox-only section) and the
uninstaller only deletes a marker-written value; smoke test asserts
OEMData absent after default install, update, and uninstall. The FH4
Wheel tab + one-click menus are user-PC proof — they cannot be
verified on Linux.

## [2.0.14] — 2026-11-26 (FH4 wheel fix)

App 2.0.14 + firmware 4.0.6 unchanged (protocol v2.2 frozen — no wire-format
change, no firmware code change in this release).
FH4 still showed no Wheel tab after v2.0.13: v2.0.13 wrote only the *required*
GameInput RacingWheel subkeys; the mapping was incomplete vs Microsoft's
example. This release writes the complete tree for both devices (vJoy
1234BEAD00010004 + HID 1209525300010004): Labels (Buttons/Switches), DPad,
Button1→0/Button2→1, Clutch→Rx/3 (clutch is now fed to Rx as well as wClutch,
so the mapping is truthful), MaxWheelAngle 450 — per the Microsoft Learn
example, value by value (unit-tested). vJoy EmuWheel envelope verified
(8 axes, 128 buttons, 1 discrete POV × 4 Directions, FFB on); envelope
validation now reports a wrong-shaped device without refusing to connect
(feeding keeps working on default-shape devices; ETS2 unaffected). Daemon
wheel-check self-check (mapping completeness + envelope + feeder state)
surfaces in the wizard Drivers step and diagnostics — plain words, no
"will work" claim. Feeder acquires device 1 and feeds neutral from backend
start, so the wheel is alive before any game launches. Installer warns
loudly when vJoyConfig.exe is missing; in-app repair (elevated) re-applies
the envelope + both mapping trees. Smoke test asserts the complete tree
after install and after the update install; packaging gate asserts the
complete tree + argv match. FH4 sends no FFB data to vJoy (Granite 2018) —
the goal is steering/pedals input, not FFB. No G29 spoof (user decision);
Steam OEMData fix never applied (breaks the Store version). The FH4 Wheel
tab itself is user-PC proof — it cannot be verified on Linux.

## [2.0.13] — 2026-11-26 (polish round, unreleased — proposal only)

App 2.0.13 + firmware 4.0.6 (protocol v2.2 frozen — no wire-format change,
one new ctrl status code: NVS_FAIL 8, SET_PASSWORD sync failure).
Backend polish batch from the 2026-11-26 deep scan (122 findings):
CORE 19, HID 15, PERF backend, INSTALLER 21, FIRMWARE 21.
Horn bit canonicalised (firmware bit 4, was read as bit 2 on the host);
pedal button-mode wired end to end; HID submit async; p99 caching;
IPC parse-once + latency without per-dispatch alloc; installer now
committed `installer/rsh.nsi` (no more 21,000-char ceiling), NO
scheduled task (autostart owner is the daemon Run key), desktop-shortcut
option, ARP fields, SHA256SUMS.txt as a 4th release asset; firmware
I2C recovery, 60 s boot-stability rule, handbrake health, mV probe
stepped, AS5600 3-byte read. Code-signing cert still NOT bought
(user decision 2026-11-13). Gate-race hardened (all-known settlement
predicate); restart-test retry widened to 10x200 ms. Bench re-measure
was host-noise-dominated (loadavg 5.5–6.9 on the shared VM) — thresholds
unchanged. Release NOT in this round — proposal only, on user order.

## [2.0.12] — 2026-11-25

App 2.0.12 + firmware 4.0.5 unchanged (protocol v2.2 frozen — no wire-format
change). Fixes the v2.0.11 user-PC retest Bug 1 (HID switch still failing —
new error "sidecar did not connect back in time") with a PROVEN root cause,
plus the user-reported Add Game bug ("games are not supported by this API").

**HID backend — token never reached the sidecar (root cause PROVEN on the user's PC).**

- **Token via argv nonce (H1).** The daemon passed the sidecar token through
  the environment (`RSH_HM_SIDECAR_TOKEN`); a non-elevated daemon launching
  the elevated sidecar via `ShellExecuteEx "runas"` goes through AIS, which
  builds a fresh environment and drops the var. The sidecar log proved it
  (`bad-args: --connect-back needs RSH_HM_SIDECAR_TOKEN`, exit 2 before
  connect). The token is now a random one-time nonce on the command line
  (`--token <nonce>`), echoed in hello and checked by `validate_hello`;
  the env transport is deleted and must not be reintroduced. The nonce is
  never logged anywhere.
- **Accept-timeout diagnostics (P0).** The v2.0.11 timeout path logged only
  the outcome — no spawn detail. It now logs spawn PID + `GetExitCodeProcess`
  exit code + elapsed ms; `hProcess` NULL / PID 0 fails fast with
  `SpawnFailed`; foreign-peer drops log expected-vs-actual PID. Reply
  timeouts (`install_driver`/`create`) now carry the bound hit + PID + exit
  code + ms since last reply instead of the bare `ReplyTimeout`.
- **Nonce/PID hygiene.** Stale single-use nonces and spawn PIDs are cleared
  on teardown (`kill_transport`), so a later failure cannot name a dead PID
  or reuse a nonce.
- **HID path sweep.** Full switch path re-reviewed (spawn → accept → hello →
  install_driver → create → live); nothing else found blocking.

**Add Game — "games are not supported by this API" (Bug 2, root cause in code).**

- **`DeferredApi` never forwarded game calls.** The IPC server holds
  `DeferredApi` permanently and installs the live `DaemonApi` into it, but
  its `AppApi` impl never forwarded `add_game`/`add_game_with_key`/
  `update_game`/`remove_game` — every Add Game click fell through to the
  trait default, daemon running, no board needed. The `.lnk` shortcut was a
  red herring. 14 methods are now forwarded (4 game, 3 Wi-Fi, 2 shifter,
  3 long-press, 2 capture); a scripted comparison of all 89 trait fns
  confirms no other IPC-dispatched method remains un-forwarded.
- **Idempotency key now actually sent.** The dispatch comment claimed the UI
  sent one — no UI code ever did, so daemon dedupe never engaged and a
  retried save could duplicate the game. One key is now minted per
  add-dialog session and sent over IPC.
- **Honest failure.** If the IPC link dies while the Add Game dialog is open,
  the UI now says RSH isn't running instead of hitting the `SimApi` trait
  default.

## [2.0.11] — 2026-11-25

App 2.0.11 + firmware 4.0.5 (protocol v2.2 frozen — no wire-format
change). Two rounds: the HID fix batch for the v2.0.10 user-PC
retest Bug 1 (sidecar died between connect and hello, 30/30 times,
with no trace anywhere), then a full-project deep scan of the user's
`rsh-update4` code drop (PR #4, 64 findings, all fixed).

**HID backend — sidecar death before hello (root cause addressed).**

- **Hello before context (P1).** The sidecar now sends its hello
  BEFORE creating the HIDMaestro context (`new HMContext()` is the
  prime suspect for the native crash — a native crash there used to
  kill the sidecar before it ever said hello). The hello already
  tolerates a null context, so a context failure is now a
  diagnosable, recoverable protocol state instead of an invisible
  fatal crash. A sidecar whose context cannot be created reports
  `context-unavailable` from install-driver / create (new
  `ContextUnavailable` error + Output-page state), instead of
  vanishing.
- **Win10 22H2 investigation (P1).** No concrete Win10
  incompatibility was found: HIDMaestro's own prerequisites say
  "Windows 10/11", and the `windows10.0.26100.0` annotation is a
  build-time API-surface marker, not a runtime OS requirement
  (Microsoft: the TFM version "doesn't control the OS version that
  your app supports at runtime"). No Win10-specific fix was
  invented; the proof needed is the Event Viewer Application-log
  entry (Event 1000) at a failure timestamp.

**Diagnostics (P0) — a pre-hello death now leaves a trace.**

- The sidecar writes a file log,
  `%PROGRAMDATA%\RSH\logs\rsh-sidecar-YYYYMMDD.log`, with startup
  milestones (connect ok, DLL-hash verdict, context result, hello
  sent) — stderr alone was discarded on the TCP path and when
  elevated.
- The daemon's `SidecarDied` message now carries the sidecar's PID,
  its exit code via `GetExitCodeProcess` (0xC0000005 = native crash,
  2 = clean exit), and the accept→EOF elapsed ms, so an instant
  native crash is distinguishable from a clean exit. It lands in
  the daemon log through the switch-failure line.
- The daemon logs its own elevation state at startup (`daemon:
  elevation: …`) — an elevated daemon (the v2.0.10 finding) is now
  visible in the log instead of inferable.
- The installer registers WER LocalDumps for `RshCoreHiDcr.exe`
  too (daemon/UI only before) — a sidecar native crash now
  produces a dump in `%ProgramData%\RSH\dumps`.

**Retry storm (P2) — explicit switches are rate-limited.**

- After 3 consecutive failed explicit switches to HIDMaestro,
  further explicit switches are refused for 30 s with "HIDMaestro
  switch failed 3 times — retry in 30 s" (the UI shows the same
  30 s cooldown). A successful switch resets the count.
- An explicit re-select now re-arms the INSTALLED backend in place
  (new `IJoystickOutput::reset_fallback` hook) instead of
  replacing it with a fresh factory build — a fresh build used to
  hand every attempt a fresh manager with a full retry budget,
  defeating the 2-failure latch and clearing the UAC-decline latch.

**Installer (P4) — no more elevated launch.**

- The finish-page "Run RSH" checkbox now launches the UI
  non-elevated via explorer.exe (the standard stock-NSIS
  technique) instead of `ExecShell`, which inherited the elevated
  installer token — the UI and the daemon it spawns ran elevated,
  so the sidecar's UAC prompt never appeared. The after-update
  daemon restart uses the same non-elevated launch.

**CI (P3) — the TCP path is tested now.**

- `check-sidecar-hello.ps1` now also spawns the staged sidecar
  with `--connect-back` + token, accepts the loopback connection,
  and asserts a hello arrives over the socket (the v2.0.10 failure
  mode passed the old stdio-only check). It also asserts the
  sidecar file log exists with the 'hello sent' milestone.
- New Rust test: a sidecar that connects and dies before hello
  must report `SidecarDied`, never hang
  (`tcp_connect_then_die_before_hello_is_sidecar_died`).

**Root-cause reframe (deep scan, PR #4) — the socket, not the crash.**

- The v2.0.10 failure is now believed to be a **nonblocking-socket
  bug, not the HMContext native crash**: the accepted loopback
  socket inherited nonblocking mode from the listener, so a healthy
  sidecar that had not written hello yet read as "died before
  hello" (`read_line_capped` maps any read error to EOF). It fits
  all the evidence (exit code 259 STILL_ACTIVE, 32 ms after spawn,
  no crash dialog, no dump; the standalone hello check passes on
  blocking .NET sockets). Fix: the accepted stream is put back to
  blocking mode right after accept (`transport.rs:327`), plus a
  real daemon→sidecar TCP end-to-end test in CI. **The user's PC
  retest remains the proof — this is not claimed proven until the
  switch works on Win10 22H2.**
- The HID double-verify also fixed the timeout budget (daemon worst
  case 310 s post-UAC vs the UI's 390 s IPC / 420 s job bound —
  a slow-but-healthy switch used to be reported failed while the
  daemon kept going), enriched the post-hello EOF with PID /
  exit code / timing like the pre-hello path, retries transient
  PID-lookup failures for up to 2 s instead of killing a healthy
  sidecar, puts the storm guard on the enable-toggle path too, and
  gives install/create 120 s and the first neutral 10 s.

**Firmware 4.0.5 (deep scan) — pedals, AS5600, config validation.**

- I2C pedals now reach 100% in-game: the pedal path converted
  raw→mV twice (steering was exempt via `steering_is_mv`; the
  pedal path was missed). Per-slot `axes_is_mv` bitmask.
- AS5600 centre-boot mapping: boot position is now 1650 mV, so a
  boot-centred wheel no longer reads all left travel as 0 mV
  (boot-at-lock was rejected as unenforceable).
- `BUTTON_MATRIX` records are rejected at COMMIT (no firmware
  consumer — the expander-gap pattern, silently dead before);
  out-of-range slots are rejected too (AXIS/AS5600 > 4, BUTTON >
  2, ENCODER > 1, LED_STRIP/BATTERY != 0).
- COMMIT persist rewritten (prv → pend → cur + read-back verify);
  handbrake latch uses the pre-ramp value; AGC is enforced. The
  engine holds the last-good axes on `axis_invalid`.

**Daemon / engine (deep scan).**

- Game writes are idempotent (idempotency key) and the IPC
  timeout no longer means "not executed"; TOCTOU in the
  remove/update game guards is fixed.
- The watchdog writes `degraded` / `ManagerLoopHung` to
  `daemon-status.json` instead of leaving the daemon half-alive
  with no signal; `HelloReceived` fires again after an activation
  failure; the pid-file claim is fail-closed; Wi-Fi password
  changes roll back on failure.
- The wizard "Join wheel Wi-Fi" flow could never succeed
  (`UnknownMethod` — the Wi-Fi scan + rejoin IPC dispatch arms
  were missing); the dispatch arms and `DaemonApi` overrides are
  added. HELLO log dedupe is keyed on (MAC, firmware).

**UI (deep scan).**

- Real 2-column games grid (broke at ≥3 games); the Modal scrim
  no longer closes the GameDialog and discards typed input
  (blocking TouchArea on Modal/popover/drawers); game save failure
  shows an inline error; the edit-game dialog pre-fills the exe
  path.
- Contrast: `ok` on Light 4.38→5.03:1, `faint` on Dark
  4.16→4.93:1 (WCAG AA); window minimums 880×480, wizard
  ScrollView; Extras cards reworked to overlap/`visible` so ON
  and OFF are the same size.

**Installer / CI (deep scan).**

- An update install now kills the running sidecar (both exe
  names) before writing files — the update used to abort with
  "Error opening file for writing" while the HIDMaestro backend
  was active.
- The sidecar hello check now runs in the release workflow, and a
  CI guard job fails if the NSIS `run:` block grows past 20,800
  chars (GitHub's limit is 21,000; it is at 20,469).

## [2.0.10] — 2026-11-24

App-only release (firmware stays v4.0.4, protocol v2.2 frozen — no
firmware change). The user's final code drop: game saving, the HID
loopback check, StatusBar lifetime, and layout alignment.

**HID backend — loopback peer check fixed.**

- `verify_loopback_peer_pid` matched the daemon's own TCP table row
  instead of the sidecar's mirror row, so the check always reported
  the daemon's PID and refused every sidecar. It now matches the
  mirror row (local=remote_port, remote=local_port) and requires both
  addresses to be loopback.

**Games — saving no longer fails as "Connection dropped".**

- Add/update game now gets a 10 s bound: an atomic file write plus a
  daemon process refresh can exceed the 500 ms default and surfaced
  as a bogus "Connection dropped".
- A failed save keeps the daemon's real reason (empty name, unknown
  profile, "cannot save manual games: …") instead of collapsing into
  the generic link failure; IPC errors now carry the detailed
  `code|reason` payload.
- A game whose save fails is no longer kept in memory — the list
  cannot show an unsaved game.

**StatusBar — errors hide after ~1 s.**

- Error lifetime 8 s → ~1 s (1 Hz tick, real range 1–2 s). An expired
  message now hides the bar (empty kind) instead of leaving an empty
  red bar on screen forever.
- A file-picker failure now includes the detail.

**Layout — rows vertically centred.**

- Every `HorizontalLayout` row across all screens now uses
  `cross-axis-alignment: center`.
- The setup wizard reserves a permanent 40 px for the floating
  StatusBar (padding-bottom 24 → 64 px) so the "Skip for now" link
  is never covered.

## [2.0.9] — 2026-11-24

App-only release (firmware stays v4.0.4, protocol v2.2 frozen — no
firmware change). Fixes the v2.0.8 retest findings: the HID backend
switch, all 20 UI report bugs, and the installer — plus the sidecar
rename.

**HID backend (v2.0.8 retest Bug 1 — root cause found and fixed).**

- **Missing DLL.** The installer never shipped `HIDMaestro.Core.dll`,
  so the sidecar's DLL-hash check failed and every HIDMaestro switch
  errored. The DLL is now staged beside the sidecar, installed with
  it, removed on uninstall, and the release smoke test fails closed
  if it is missing after install.
- **Full diagnostics.** The sidecar hello now reports `dll_sha256`
  and `dll_sha256_ok`; every backend-switch outcome is logged to the
  daemon log; Diagnostics shows the full notification body instead of
  just the title. A failed switch now says why.
- **Windows 10 22H2 is supported.** HIDMaestro v2.1.0 is tested on
  Win10 IoT Enterprise LTSC (build 19044); the user's PC (19045) is
  newer. WDK 26100 is a build requirement, not a runtime minimum.
- **CI check.** A new `check-sidecar-hello.ps1` runs the staged
  sidecar and asserts SDK 2.1.0 + a matching DLL hash.

**Sidecar renamed: `rsh-hm-sidecar.exe` → `RshCoreHiDcr.exe`.**
The old name was unprofessional. Renamed end to end (csproj,
staging, NSIS, daemon resolve path, docs); uninstall removes both
names so an upgrade from ≤ v2.0.8 leaves no stale exe. The
wire-protocol hello identity `"rsh-hm-sidecar"` is unchanged — it is
a protocol value, not a filename.

**Installer modernized (MUI2).** Welcome / License / Directory /
Install / Finish pages with RSH header + sidebar bitmaps and a
"Run RSH" checkbox on Finish. Smart install/update/detect,
firewall rule, clean uninstall and registry keys are unchanged.
Also fixed: the WER crash-dump folder used `$PROGRAMDATA`, which
is not an NSIS constant — dumps went nowhere since Round 3. NSIS
is now pinned to 3.13.0 with a fail-closed version check.

**UI — all 20 retest-report bugs (B01–B20) + design fixes (D1–D12).**
The floating toast is replaced by one 40 px bottom StatusBar
(busy/error/success, full untruncated text, Details drawer);
Buttons page gets its own scroll + edit panel; Steering panel
widened; type scale, spacing and icons are theme tokens / Lucide
SVGs; the game dialog is a real centred Modal; sidebar labels say
"Wheel not found" / "Disconnected" instead of "Unreachable" /
"Timeout".

**Security advisory.** `cookie` 0.18.2 → 0.18.3 (RUSTSEC-2026-0594,
transitive via Tauri).

**CI/gate.** The `ui-banner-wrap` probe hardened: its 168-char text
wrapped to exactly 2 lines and sat on the 46 px min-height under
the CI fallback font, so the check flipped with the font. 318
chars force 3 lines.

## [2.0.8] — 2026-11-23

App-only release (firmware stays v4.0.4, protocol v2.2 frozen — no
firmware change in this range). Two things land together: the 8 bugs
found on the user's own PC test of v2.0.7, and per-module tuning
features for every module.

**PC-test bug fixes (user reported 2026-11-22, all 8 fixed).**

- **False "connected" toast (Bug 1).** The "RSH connected / Live data
  is back" toast fired when the UI↔daemon feed resumed, even with the
  board offline. It now fires only when the board is actually
  connected, and says "Wheel connected".
- **Huge toast (Bug 2).** A height-less Slint rectangle stretched to
  the full window height on Windows. The toast now has an explicit
  height (~53 px short / ~69 px long) with title/body length caps.
- **Clipped "Wheel not found" banner (Bug 3).** The banner text
  wrapped and its second line was cut off on Windows (Inter font).
  Both Overview texts are shortened to fit one line at window width,
  with regression guards. Final proof needs the user's PC (the clip
  never reproduced on Linux).
- **Controller switch stuck (Bug 4).** Switching vJoy → HIDMaestro
  stuck on "Waiting for admin permission…" even after UAC was
  granted: the IPC error reason was dropped in two places and the
  state error never matched `HID_NOT_ADMIN`. The reason now survives
  end to end, the time-based relabel is gone (the label honestly
  says "Switching controller…"), and a failed switch shows one clear
  error.
- **Big warning popups (Bug 5).** The same height defect made every
  overlay a full window. All overlays now have explicit content
  heights; all 109 notify call sites go through the one small toast.
- **Add game (Bug 6).** The manual text form is gone: Add now opens
  the native Windows Explorer picker (`IFileOpenDialog`, .exe/.lnk
  filter) and the name auto-fills from the file.
- **Steering Advanced (Bug 7).** The useless read-only rows (Centre
  mV, Raw live, curve exponent box, min/max note) are removed.
  Advanced is now real tuning: Invert steering toggle, Steering lock
  270/360/540/720/900°, Smoothing 0–100% (default 30%), and the Basic
  deadzone cap widened to 30%. No second Sensitivity slider — the
  Basic Response curve already is it.
- **Wrong-backend switch (Bug 8).** A failed HIDMaestro activation
  silently fell back to vJoy and the UI reported success ("now
  vJoy"). An explicit switch now never falls back: a mismatch
  between requested and active backend is a failure with a clear
  error, and the Settings radio resyncs to the live backend.

**UI polish.** Steering Advanced Invert row centre-aligned, row gaps
matched to the Feel card, "Recommended: vJoy" spacer, GameDialog
file row centre-aligned. No design change — the approved preview is
still the source of truth.

**HIDMaestro LICENSE (GitHub issue #2).** The installer now ships
`HIDMaestro-LICENSE.txt` beside `rsh-hm-sidecar.exe`, and the release
smoke test fails closed if it is missing.

**Per-module tuning — every module gets its own settings**
(researched against Fanatec / Moza / Logitech G HUB / SimHub /
iRacing / ACC; nothing is copy-pasted between modules).

- **Steering:** Capture full lock (left/right buttons), left/right
  lock balance ±20%, centre trim ±5%.
- **Brake:** Brake force (BRF) 50–100% (new `saturation` knob, so
  pedal calibration is untouched), bite point, progressive curve
  (default 1.3), initial bite speed.
- **Accelerator:** Tip-in / lift-off ramps, throttle curve,
  saturation, combined-pedals toggle (one axis, for old games).
- **Clutch:** Bite point (kept), engagement width / slip zone
  5–40% (new `bite_width`), release speed, curve.
- **Handbrake:** Latch mode, auto-release on throttle, grab speed.
- **Shifter:** Sensitivity slider wired up (was disabled; writes
  the firmware config blob, debounce 20–5 ms) plus shift lockout
  0–200 ms.
- **Buttons:** Debounce 0–50 ms, long-press action, double-press
  action. Hold-to-repeat was NOT built: a held vJoy button already
  repeats in the game's eyes, so the semantics need a product
  decision first.
- **Encoders:** Pulse length 20–200 ms (games polling slowly no
  longer miss steps), steps per click 1/2/4 (default 4),
  acceleration, per-knob direction invert.
- **LED:** Blink / chase speed 100–5000 ms, custom colour picker.
- **Horn:** Minimum honk duration 100–1000 ms, Momentary / Latch
  mode (default Momentary).

Skipped on the user's order: LED RPM / shift-light and per-LED
zones (they need a game-telemetry subsystem that does not exist
yet). Also fixed along the way: the `gate-already-running-race`
flake (pid-file write is now awaited before a claim is reclaimed)
and a clippy `struct_excessive_bools` in the engine pipeline.

**Real bugs found (and fixed) while building tuning.**

- Throttle settings silently never reached the engine: a spelling
  split meant throttle deadzone / Feel edits were dropped. If you
  tuned the throttle before this release, check your throttle
  settings again.
- Encoder module-level invert was silently dead (per-knob invert
  now works).
- Capturing full lock after a centre capture failed (min == max ==
  centre).
- Pid-file claim hole: a losing claimant could delete the winner's
  empty pid file and become First itself.
- Throttle saturation (A4) reset to 100% on a Set-down range
  capture — by design; re-set it after re-calibrating.

**Not yet verified / follow-ups.** Windows-only behaviour (pedal
feel, vJoy / HIDMaestro, Slint rendering, board round-trip,
shifter config write) can only be proven on the user's PC.
Handbrake button-mode debounce still follows shifter sensitivity
(`inputs.cpp:127` — firmware-config fix, out of this scope).
UI-spawned daemons silently die after ~11 s (pre-existing, cause
unknown — follow-up). The `gate-already-running-race` hardening
proposal is still due (the flake has now hit 5 times).

## [2.0.7] — 2026-11-22

**Expander guard (Round 3, post-consolidation).** MCP23017/PCF8574
expander-button sampling is not implemented, and the validator used
to accept expander records — a user configuring expander buttons got
COMMIT OK and then silently dead buttons. Both sides now reject
expander records at COMMIT with a clear error: the firmware
validator (`UNSUPPORTED`, err 3) and the app (`validate_blob`, plain
message) before anything reaches the board. Axes (ADS1115/AS5600
sampling, FW-01) are unaffected. A board with a legacy expander blob
boots on factory defaults, never bricks; factory reset clears it.

**Anti-kill / anti-crash (WS-6, R3-AK-01–04).** Desktop Windows does
not kill Win32 apps for resource pressure — the real gaps were
respawn, crash forensics, tray survival, and OS restart
registration (see `deep-scan/ANTI-KILL-RESEARCH.md`).

- **Task Scheduler supervisor (R3-AK-01).** The installer creates a
  "RSH Daemon" scheduled task (logon trigger + restart on failure:
  every 1 min, up to 3 attempts) that relaunches the daemon when it
  dies while the UI is closed. The UI respawn stays the primary path
  while the UI is open. A `--scheduled-respawn` launch exits 0 while
  a fresh fatal `daemon-status.json` exists (`IpcBindFailed` /
  `DaemonHung`, 600 s latch), so a fatally-broken daemon is never
  respawned in a loop. Removed on uninstall. The daemon never
  creates its own task at runtime.
- **WER LocalDumps (R3-AK-02).** The installer registers
  `HKLM\...\Windows Error Reporting\LocalDumps\rsh-daemon.exe` and
  `rsh-ui.exe` (mini dumps, DumpCount 5) to `%ProgramData%\RSH\dumps`,
  so a native SEH crash leaves a dump the Rust panic hook could never
  produce. Removed on uninstall (dumps kept). The diagnostics export
  now lists the dumps present (names only).
- **Tray survives Explorer restarts (R3-AK-03, verified, no code).**
  Slint 1.18.1's own tray backend already handles `TaskbarCreated`
  and re-adds the icon — verified against the vendored
  `i-slint-core-1.18.1` sources, nothing to build.
- **Application Restart (R3-AK-04).** Daemon and UI register
  `RegisterApplicationRestart(..., RESTART_NO_PATCH |
  RESTART_NO_REBOOT)` + a recovery callback. Honest limit: NOT a
  silent auto-restart — WER needs ≥ 60 s uptime and the user's
  consent on crash/hang; a rapid re-crash skips re-registration.

Rejected (verified, not built): whole-process HIGH/REALTIME
priority, min working set / `VirtualLock`, job objects, protected /
critical process, DACL-deny-terminate, watcher-process exe, Windows
Service, Game Mode registry hacks, in-process minidump, MMCSS,
memory priority above NORMAL. `BackgroundGuard` untouched (user
order). Windows install/runtime proof still pending (no Windows
machine in this environment — CI smoke test + human gate).

## [2.0.6] — 2026-11-20 (in progress, not released)

**Pairing abolished — connect-play instead (feature, not a bugfix).**
User order 2026-11-20: the pairing system is removed outright and
replaced by a simple connect-play system. No release yet; work is
in progress on the `no-pairing-*` branches.

- **No device key, no HMAC, no pairing window.** Protocol v2.1 →
  v2.2 (version byte `0x22`): control frames are plain
  (`"RSHC" | cmd | reqId | len | payload | CRC16`); duplicates are
  caught by reqId only (last 32); `BAD_TAG` deleted; `KEY_RESET`
  opcode 0x0B deleted. HELLO stays 92 B — its key/password/nonce
  fields are reserved/zero and never disclose a secret.
- **Connect-play.** The board opens an open hotspot `RSH-XXXX`
  (last 4 hex of the board Wi-Fi MAC); the PC joins it (or USB),
  the app connects. The user sets a Wi-Fi password in
  **Settings → Connection** (8–63 printable ASCII); later
  connections use it. `SET_PASSWORD` carries the current password
  when one is set. Forgot it: hold BOOT ≥ 10 s — the firmware
  clears the password itself and reboots to an open AP (no wire
  command, no armed window).
- **WebOTA disabled until a password is set.** WebOTA auth is the
  user-set AP password (one credential); per-IP rate limit +
  Ed25519 verify + anti-downgrade kept.
- **Wizard is 4 steps** (Connect → Drivers → Calibrate → Games);
  Step 1 Next unlocks when the board is reachable (HELLO or
  better), never on pairing. Connection state machine is
  `Idle → LinkUp → Hello → Active`; `Active` = unsigned `GET_CAPS`
  OK. `board_reachable` latch drives the wizard and the banner.
- **Migration:** update the app first (v2.0.6), then flash firmware
  4.0.3 via WebOTA with the old stored Wi-Fi password; 4.0.3 first
  boot deletes the old NVS secrets (`devkey` / `paired` / `wifipw2`
  / `wifipw`) and brings the AP up open. Old firmware (≤ 4.0.2)
  still parses so the app can offer the update; an old app cannot
  talk to firmware 4.0.3.
- **Docs:** new `docs/PROTOCOL-V2.2.md` (wire authority);
  `docs/KEY-LIFECYCLE-RUST.md` deleted; `THREAT-MODEL-RUST.md`
  rewritten (the WPA2 password is now the only boundary);
  `SECURITY-INPUT.md` and `ARCHITECTURE.md` updated; docs/ stays
  at exactly 6 files.
- **Rig:** pairing scenarios deleted; new connect-play scenarios
  (`connect-open-ap`, `connect-password-set`,
  `connect-password-reconnect`, `connect-key-reset-open`,
  `connect-usb`, `connect-migration-402-403`, `connect-unreachable`);
  HMAC benches deleted.

## [2.0.5] — 2026-11-20

Pairing confirm fix + full UI polish + cards layout + performance
(commit `b897188`, `a935db2`, `02bfced`):

- **Pairing fixed (research-first).** Root cause: the confirm block
  only showed while `pairing-active` was false, but every attempt
  sets it true until the 60 s window expires — confirm could never
  be clicked. Now the replace block shows whenever a replacement is
  required and the board is unpaired; "Replace pairing" is the
  primary action and reset actually deletes the stored key; the
  Overview banner offers confirm directly; the one-shot flag is
  board-bound; failure clears `pairing-active` and re-offers the
  block. Wi-Fi join banner + first-Unreachable notification.
- **UI polish.** Every popup closes on outside click; errors only as
  a centred ~480 px toast, a small inline box, or a banner — never
  full-window. UI logging added (`rsh-ui-YYYYMMDD.log` in
  `%APPDATA%\RSH\logs`).
- **Cards layout.** Gaps now auto-adjust to the window (spacers +
  min-width); cards are the same size ON and OFF (body stays in the
  tree, greyed when off).
- **Performance.** Release profile (strip + LTO fat): binaries
  −36.8% (daemon 36.65 → 22.05 MB, UI 42.33 → 27.90 MB). Frame rest
  (unchanged frames skip ~52 set calls; 10 Hz when not live);
  wakeup sources cut (USB-only port refresher; slow-poll 192.5 µs →
  9.3 µs); buffered UI log; tray refresh gating; window shown
  before the sync block, first paint logged; second job lane;
  backend lock fix; new perf gate step.

## [2.0.4] — 2026-11-19

Round 2 deep-scan fixes (51 findings: 2 P0, 24 P1, 25 P2), merged as
five workstreams (merge `332f341`, gate fixes `c9a1766`):

- **Game auto-scanner fully removed.** Games are now added manually
  (name + exe/shortcut) and launched locally. No more Steam/iRacing
  auto-detection, no registry scanning, no `reg.exe`/`tasklist` on
  any hot path.
- **Setup wizard fixed.** Step 1 board-connect check (Next unlocks
  when the board is reachable), Step 2 drivers check, Step 3
  pairing. Stored-key case now offers confirm-replacement instead of
  a dead end; the 10 s Begin-pairing wait works.
- **Connection state machine fixed.** The manager reaches `Active`
  for a paired board; `fail()` arms the backoff; retry works from
  `Error` instead of latching there.
- **Tray popup stays visible** (5 s minimum); tray registry spawn
  fixed; perf: firmware check cached, vJoy cached, `set_tuning` no
  longer triggers a full refresh; background resilience
  (power-throttling opt-out, power request while the wheel is
  Active).
- **UI/UX polish.** Spacing pass, live pedal graphs at 20 Hz,
  per-button busy states, loading and error states, theme handling.
- **Release:** NSIS installer + firmware v4.0.2 (merged full-flash
  image + Ed25519-signed OTA image). Three assets.

## [2.0.3] — 2026-11-18

- Deep-scan fixes (DS-01–DS-82): IPC pipe loop fixed, window-close
  crash (SIGSEGV) fixed, daemon respawn loop fixed, exit codes
  fixed, reconnect-failed spam stopped.
- vJoy is the default backend. First-run wizard (5 steps, drivers
  step). Tray + first-run wizard.
- **Release:** installer + firmware v4.0.2 (merged + signed OTA).
  Three assets.

## [2.0.2] — 2026-11-17

- First three-asset release (installer + merged firmware + signed
  OTA image for the app's WebOTA flow).
- Smart installer: detects an existing install and offers
  update / reinstall (with a downgrade warning).

## [2.0.1] — 2026-11-17

- Hotfix: daemon self-spawn + live IPC, fake sim data removed (real
  disconnected state), all buttons wired, tray + first-run wizard
  (5 steps, drivers step), vJoy default backend.
- **Release:** installer + firmware image. Two assets (OTA added in
  v2.0.2).

## [2.0.0] — 2026-11-16

- v2.0 redesign: Slint UI + always-on daemon (Tauri/React gone).
  Three nav entries (Overview, RSH Wheel, Settings), dark theme,
  920×580 window. Firmware v4.0.2, protocol v2.1 frozen.
- **Release:** installer only. Single asset.

## [1.0.2] — 2026-11-15

- Hotfix: startup failures are now visible (native error dialog +
  dual-location startup log); no console window on launch (app +
  sidecar are GUI-subsystem); launch path smoke-tested in CI.

## [1.0.1] — 2026-11-15

- Hotfix: no console window on launch (app + sidecar
  GUI-subsystem).

## [1.0.0] — 2026-11-15

- First Rust release: Tauri 2 app (NSIS + MSI) + C++ firmware
  v4.0.1 (signed). Full Rust recreation of the C# app; protocol
  v2.1 frozen; Ed25519-signed firmware.
