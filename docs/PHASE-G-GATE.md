# Phase G Gate — Output backends

**Verdict: CONDITIONAL PASS** (2026-11-14, coordinator gate round)

Phase G builds G0–G4 per the architecture + software plans (SW-R6).
All sub-phase logic is built, wired, and tested. The conditions below
are all named, owned, and carried into Phase H — none is a hidden
defect.

## What was built

- **G1 HIDMaestro interop spike** (`docs/HIDMAESTRO-SPIKE.md`, 329
  lines): decision **GO — path (b) C# NativeAOT sidecar**
  (`rsh-hm-sidecar.exe`, JSON-lines over stdin/stdout). Direct
  shared-memory from Rust is NO-GO as primary (no stable native
  contract for install/device-create; DACL gives non-elevated
  processes read-only access). Pinned artifacts re-verified:
  ZIP SHA-256 `7fe349f1…397b4`, DLL `f13e3259…4d5`; v2.1.0 still
  latest. Catalog: 336 profiles load; **no generic wheel profile
  exists** — all 50 wheel profiles are brand identities (spoofing
  rejected); custom profile `rsh-wheel-custom` proven to build
  (5 axes X/Y/Z/Rx/Slider, 32 buttons, 124-byte descriptor in 3 ms).
  IPC latency at 200 Hz: p50 69 µs / p99 276 µs / max 1.23 ms (~35×
  headroom vs the 10 ms E2E p99 SLO). NativeAOT cannot build on
  Linux (measured) — AOT publish must happen on Windows.
  **Headline caveat: an elevated helper for the whole session — a
  UAC prompt on every RSH launch when the HIDMaestro backend is
  active.** vJoy needs no runtime prompt.
- **G0 AppState entry item** (Phase F condition 1):
  `AppStateCore` (`rsh-app-core/src/state.rs`) — engine metrics +
  `LoopRateTracker` + `EncoderState`, backend slot
  `Mutex<Box<dyn IJoystickOutput>>` (NullSink installed), `GameLauncher`,
  `ProfileStore`; Tauri `AppState` (`.manage()`); 25 new thin stateful
  wrappers; `IMPLEMENTED_COMMANDS` 10 → 35, `NOT_YET_IMPLEMENTED`
  44 → 19. 30 Hz `rsh://snapshot` emitter (33 ms, skips missed
  ticks, never bursts), `rsh://button-press` edge diffing (physical
  1–64 + encoder pulses 65–72), real shutdown-neutral path with the
  honesty guard (`neutral_sent` only when the backend was connected
  AND the call succeeded).
- **G2 HIDMaestro backend** (`crates/rsh-output-hidmaestro/`, 14
  files): sidecar process manager (spawn → hello validation → install
  → create → submit; heartbeat 1 s, EOF/crash detection, **one retry
  then fail-fast `FallbackNeeded`**), `HidMaestroOutput`
  implementing G3's trait as defined, frame mapping routed through
  vJoy-unit axis ints so both backends agree exactly, stub sidecar
  for Linux tests. Production sidecar `tools/rsh-hm-sidecar/` (10
  files): AOT-safe source-generated JSON (**zero IL3050/IL2026
  warnings**), `app.manifest` (`requireAdministrator`), DLL-missing
  fail-fast, `PROTOCOL.md`, `verify-linux.sh` PASS. **Raw report is
  15 bytes, not 26** (derived from the real descriptor; the C# 26-byte
  assumption was a never-built 128-button profile). Default path is
  `SubmitState` (buttons 1–25); raw (32 buttons) opt-in.
  VID `0x1209` / PID `0x5253` ("RS") — **PROVISIONAL** until a
  pid.codes PR claiming `0x5253` merges (release gate).
- **G3 vJoy FFI backend** (`crates/rsh-output/src/vjoy.rs`, 2,894
  lines): hand-written `extern "C"` bindings via `libloading` (no
  `vjoy` crate, no bindgen); `JOYSTICK_POSITION` repr(C) 124 bytes
  with 17 offset asserts; mapping matches C# `InputMapper` exactly
  (incl. banker's rounding, NaN/inf → neutral); buttons 1–32 + Ex1/Ex2
  encoder pulses (D9: never clamp); D11 capabilities validation;
  missing-driver journey (honest "vJoy not found" + install link).
  Pin: BrunnerInnovation **v2.2.2.0** via `DriverMatch`. Keepalive
  (D32) dropped + documented — the Phase B engine already re-sends
  every 5 ms tick.
- **G4 backend selector** (`crates/rsh-output/src/selector.rs`):
  HIDMaestro default, vJoy fallback, None/NullSink; runtime switch
  with **no restart**, connect-first-then-disconnect (never
  half-connected); HIDMaestro failure → vJoy with the reason recorded
  verbatim; both failing names both attempts. `set_output_backend`
  is now real (persists the choice first, then switches);
  `list_output_backends` returns per-backend status (selected/active/
  connected, install probes, elevation + UAC note, vJoy download
  link). Settings default is now `"HIDMaestro"`. Output page picker
  (live-backend radio group, badges, UAC note, fallback banner,
  honest switch errors; en/ur/hi key parity). `docs/FFB-EXTENSION.md`
  (159 lines, documented only — FFB is NOT in v1.0.0; HIDMaestro HID
  PID extension point + 7 steps; vJoy has no path).
- **Gate integration**: engine task spawn wiring (transport
  `PacketSource` → engine → output; UDP startup default, serial
  auto-detect; forwarder rebuilds every 2 s after loss; clean idle
  state with no transport — watchdog stale → neutral, no fake
  frames); startup auto-select of the persisted backend (HIDMaestro
  → vJoy → NullSink, reason recorded, never silently on NullSink);
  `docs/GAME-MATRIX.md` per-backend draft (16 pairs, **0 of 16
  verified — Windows-only**).

## Verification (gate re-run by the coordinator)

- `cargo build` 0 errors (via test + clippy runs).
- `cargo test --workspace --exclude rsh-app`: **716 passed / 0
  failed** (32 suites, 3 ignored pre-existing) — Phase F was 475;
  Phase G added ~241 tests (protocol 60, engine 79, output 93,
  hidmaestro 118, app-core 102, profiles 45, transport 18,
  security 10, telemetry 39, + others).
- `cargo clippy --workspace --all-targets --exclude rsh-app
  -- -D warnings`: clean.
- `cargo fmt --check`: clean.
- Frontend: `npm test` **196/196** (18 files), `tsc --noEmit`
  clean, `eslint` clean, `npm run build` passes (4.39 s).
- `cargo deny check`: advisories / bans / licenses / sources all ok.
- `cargo audit --deny warnings`: yanked check timed out again
  (registry slowness in sandbox — same as Phases E/F; re-run on CI /
  Phase H gate).

## Bugs found and fixed during the gate

- **G-G1 (encoder over-count, gate integration):** the engine
  repeats `last_output` (deltas included) on ticks with no new
  frame, so accumulating encoder deltas in the sink over-counted
  10× (100 instead of 10). The feeder now reports the board's
  cumulative encoder counts per frame (`note_frame_encoders`);
  repeats diff to zero in the snapshot tracker.
- **G-G2 (game-id/slug mismatch, G0):** launcher game ids are
  hyphenated (`assetto-corsa`) but profile slugs underscored
  (`assetto_corsa`), so bound-profile lookup could never match —
  fixed with normalization in `list_games`.
- **G-G3 (vJoy DLL SHA-256 missing):** no hash exists in any source
  (plans, RESEARCH.md, C# sources all searched) — `PINNED_DLL_SHA256_HEX`
  is deliberately `None` with the enforcement machinery ready
  (`verify_dll_sha256()` tested against the FIPS "abc" vector).
  The hash must be measured on the Windows release machine
  (`Get-FileHash vJoyInterface.dll -Algorithm SHA256`) and filled
  in — never invented.
- **G-G4 (fmt):** workspace `fmt --check` failed during G2's work —
  the diffs were entirely G2's mid-edit files; G2 formatted before
  finishing (0 diffs at the gate).

## Gate decisions (under delegation)

- **D-G1:** startup auto-selects the persisted backend (G4's
  follow-up resolved — honor the persisted choice at launch; the
  UAC prompt for HIDMaestro is expected, documented, and surfaced
  on the Output page).
- **D-G2:** vJoy stays the fallback (G1 spike was GO for HIDMaestro;
  had it been NO-GO, vJoy would have become primary per SW-R6.2).
- **D-G3:** VID `0x1209` / PID `0x5253` provisional — the pid.codes
  PR claiming `0x5253` for "RSH — Real Sim Hub Wheel" is a **release
  gate before v1.0.0**.
- **D-G4:** `OutputError` split stays, documented precisely in
  `rsh-output/src/error.rs` (a `From<HidMaestroError>` cannot live
  in `rsh-output` — circular dep — or `rsh-app-core` — orphan rule;
  it belongs in `rsh-output-hidmaestro/error.rs`). The shared
  `OutputError` type exists as the selector/diagnostics-facing
  unified type with the 3-step trait-migration path documented.
- **D-G5:** `IJoystickOutput` trait shape is G3's definition
  (`try_connect` / `try_connect_with_plan` / `disconnect` /
  `update` / `send_neutral`, `ProcessedState =
  rsh_engine::pipeline::OutputState`) — G0/G2 consumed it as
  defined; no competing trait exists.

## Conditions (carried, all named)

1. Tauri shell compile + window-paint smoke test — Windows-only
   (carried from Phase E; `src-tauri` does not compile in this
   sandbox — `gdk-3.0`/WebKitGTK missing, not a code error).
2. First green CI run only after GitHub push — push only on the
   complete project (carried).
3. OPEN-13/14 (one credential Wi-Fi+OTA) still open (carried).
4. Windows-runtime verification open: keyring trap, serialport
   921600 + DTR no-reset, `windows` crate `runas` path (not compiled
   in sandbox), real sidecar + driver (install/create/submit),
   NativeAOT publish, UAC behaviour, real vJoy DLL load/acquire/
   update, registry probe, game matrix 0/16 (carried).
5. UDP sendto blocked in sandbox (rig needs a network-capable
   runner) — carried.
6. a11y (axe) audit needs a browser: Windows/manual, not run yet —
   carried.
7. `cargo audit` yanked check timed out (registry slowness) — re-run
   on CI and at the Phase H gate (carried from Phase E).
8. vJoy DLL SHA-256 must be measured on the Windows release machine
   (see G-G3) — never invented.
9. pid.codes PR for `0x5253` is a release gate (see D-G3).
10. H-HID-1–H-HID-6 owner-PC pending; H-HID-7 ARM64 unsupported
    (stays); H-HID-8 release-notes entry pending.
11. Engine metrics unmeasured in snapshots: the engine exposes
    metrics only in its final report, so the sink feeds
    `ingest_frame` with empty metrics (jitter/loss/E2E p99s stay
    `null`, `seqGap`/`txFail` stay 0); loop rate IS genuinely
    measured. Needs a per-tick metrics outlet in the engine API —
    Phase H item (or v1.0.1, with reason).
12. Status packets are parsed by the transports but not forwarded to
    the engine (`btnsEx` stays 0) — needs a status pump (session
    layer) — Phase H item.
13. **No HMAC session gate: frames flow as soon as a transport is
    up; the `GET_CAPS` stream-emit gate (SEC-E-14) is NOT enforced
    — must never be presented as paired/secure.** Phase H security
    re-scan (H1) must cover this explicitly.
14. Pipeline runs the default plan (clutch disabled); per-module
    tunings from the active profile are not yet pushed into the
    pipeline — Phase H item.
15. Transport/pairing Tauri commands remain unregistered (no
    `BoardSession`/session gate yet) — 19 commands still
    NOT_YET_IMPLEMENTED (transport/pairing, config staging, pin-map,
    firmware upload/version, `restart_wheel`, `open_config_folder`).
    Phase H must wire them or explicitly defer each with a reason.
16. Corrupt-store startup is fail-closed (process exits, backups
    survive) — a repair UI is a Phase H item.
17. TCP connect-back from .NET processes is sandbox-blocked (egress
    proxy refuses it — proved with a minimal .NET TCP client);
    stdio path fully verified instead; TCP must be exercised on
    Windows (H-HID-5).

## Process note
The gate-integration worker ran one read-only `git diff --stat`
mid-task despite the no-git instruction — no state changed; no
further git commands were run. The Phase G commit below is the
coordinator's.
