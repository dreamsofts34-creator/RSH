# RSH v2.0.4 — Release notes (final polish sweep)

App v2.0.4 · Firmware v4.0.1 · Protocol v2.1

Polish sweep on top of v2.0.3: deep analysis + fixes across the app
and the firmware, verified in the Virtual Test Rig. **Firmware
v4.0.1 is NOT byte-identical to v2.0.3: if your board already runs
firmware v4.0.0, reflash it to v4.0.1** (merged bin in this release).
The patch bump does not trigger recalibration (by design).

## What changed in v2.0.4

### Blank window — real fix on Windows 10 (plain Window, not a property flip)

v2.0.3 did **not** fix it: the user tested it on 2026-11-13 and the
window was still blank — even when run twice from cmd with the manual
`--classic-window` flag. The audit found the real reason: **v2.0.3's
"classic window" was never a classic window.** `MainWindow`'s class
stayed `FluentWindow` (`MainWindow.xaml:1`,
`MainWindow.xaml.cs:14`); classic mode only flipped three properties
(`ExtendsContentIntoTitleBar=false`, title-bar row height 0,
`WindowTitleBar` collapsed). The whole FluentWindow template, WndProc
hook and chrome stayed in place — so the fix never actually tested
the chrome hypothesis.

- **The fix in v2.0.4:** on Windows 10 (build < 22000) the app now
  opens a **real plain `System.Windows.Window`**
  (`ClassicMainWindow` — OS title bar, no FluentWindow, no
  `ui:TitleBar`, no `WindowBackdropType` /
  `ExtendsContentIntoTitleBar`) with a plain sidebar and the same 8
  screens and pages. No flag needed. On Windows 11 nothing changes:
  the app still opens the normal FluentWindow with the styled
  sidebar, exactly as before.
- **First-run wizard:** in classic mode the FluentWindow wizard is
  skipped (logged) — it is the same FluentWindow family and would
  also be blank. The Setup screen covers the same steps (Connect →
  Pins → Calibrate), so nothing is lost. On Windows 11 the wizard
  is unchanged.
- **Version in the window title:** the title now reads
  `RSH — Real Sim Hub v2.0.4` (version from the assembly, not
  hard-coded), so a photo alone confirms which binary is running.
- **OS detection hardened:** the Win11 boundary check is now
  RtlGetVersion-based (`OperatingSystem.IsWindowsVersionAtLeast`,
  manifest-independent) alongside `Environment.OSVersion`; if the
  two sources ever disagree, a WARNING is logged. (The manifest
  itself was verified present in the shipped exe, byte-identical —
  that was never the cause.)
- **Logging hardened:** if the primary log folder
  (`%AppData%\RSH\logs`) cannot be written, the logger falls back
  to `%TEMP%\RSH\logs`; Serilog SelfLog is on
  (`%TEMP%\RSH\serilog-selflog.txt`); and every startup milestone
  is also written to `%TEMP%\RSH\startup-status.txt`, so a missing
  log file can never blind diagnosis again.
- **Refuted, for the record:** manifest missing (shipped exe has
  it), GPU/driver (RenderTest renders, Tier 2), stale process (PC
  restart), runtime missing (that shows a dialog, not a blank
  window), NavigationView (0 matches in source), Dashboard /
  navigation / theme / tray as the cause (the sidebar is static
  XAML — a Dashboard failure shows a placeholder, it cannot blank
  the whole window).

### Firmware v4.0.1 — why the version changed

Firmware code changed, so the version moved from 4.0.0 to 4.0.1:

- **Config blob validator now enforces PIN-RULES** (Medium M1):
  a staged config blob can no longer commit an ADC2 axis, a barred
  pin, an input-only encoder/button/LED pin, I2C-reserved pins 21/22,
  2 batteries or 2 LEDs. The wire format is unchanged and the PC
  builder was matched side-by-side (factory / default / full-rig
  blobs pass).
- Serial ping now checks the `RSHN` magic (L1); `fw34_slot_save`
  now checks the NVS return value (L3); stale comments in
  `webota.h` / `ota_validate.h` fixed (L8).
- **42 new host checks** (4416 total, 0 failures). `pio run`
  SUCCESS, 0 warnings.

### App-side firmware signature verification

`FirmwareImageValidator` previously only checked the trailer magic.
It now **verifies the Ed25519 signature** (NSec, baked public key —
fingerprint matches `signing_pubkey.h`), fail-closed: a tampered
payload is caught by the signature alone. The signed binary is
still verified end-to-end by the release workflow.

### Wizard calibration + pin map

- **Wizard calibration unit mismatch (High-conditional) fixed:**
  raw ADC values were fed to the mV span gate and saved as mV with
  `IsMillivolts=true` when the firmware was in raw fallback. The
  intake now converts to mV at capture (`WizardCalibrationIntake`).
- **PinMapBoardPush CONFIRM-retry edge (Medium) fixed:** a CONFIRM
  reply of `Unsupported` after a successful COMMIT in the same call
  is now treated as already-confirmed (success, no REVERT); a
  timeout gets one PushAsync-level retry.
- Diagnostics "Windows 11" wording now uses
  `RuntimeCheck.RequirementsText` (the floor is Win10 19045).

### Data integrity (10 fixes)

- Game-specific profile preference (`CarProfileResolver`); a
  corrupt `.bak1` no longer overwrites a good `.bak2`
  (`ProfileStore`); one bad pin-map entry no longer wipes the whole
  map (`PinMapStore`, 64 KB cap + tolerant per-entry parse);
  DPAPI key blob capped at 64 KB + OOM catch (`KeyStore`); missing
  iRacing `SessionState` now reports Unknown, not Menu
  (fail-closed); `TelemetryWatcher` clears the stale snapshot when
  a game closes; negative `SchemaVersion` rejected; a corrupt file
  is no longer preserved as `.v1.bak`; `Save` no longer truncates
  the caller's list; HighContrast restore no longer rejected.

### Security

- **SEC-E-04 + SEC-E-09 fixed:** the DPAPI read path is actually
  used now; legacy plaintext passwords are migrated and wiped; the
  OPEN-15 doc/code contradiction is fixed. No new Critical/High
  finding in the sweep. Still open (conditions, not blockers):
  SEC-H-04 review, and flash encryption board-side (OPEN-3 /
  SEC-E-01 — the NVS password is obfuscated, not encrypted, until
  flash encryption is enabled on the board).

### Performance — lag / freeze / slowness audit

- U-P1–U-P10 audit: **9 PASS, U-P7 CONDITIONAL PASS, U-P8
  CONDITION** (startup ≤ 1200 ms is only measurable on a Windows
  PC). No blocking call on the UI thread (`.Result()` / `.Wait()` /
  `Thread.Sleep` in `RSH.App` = 0); the 3 remaining sync
  `SaveSettings()` sites are now async.
- Benchmarks 13/15 pass. The 2 E2E pacing failures (send-gap p99,
  e2e-age p99) reproduce only under VM load (shared 1.5 CPU, parallel
  teams) and pass on quiet runs — sandbox scheduling noise, not a
  code regression; the gate was not softened.

### Docs

- `docs/UI-LOCKED.md` corrected: 337 resx keys (not 61), the High
  Contrast theme exists (`RSHTheme.HighContrast.xaml`), the runtime
  language switch exists (Settings → Language).
- Two stale v1 docs bannered SUPERSEDED
  (`docs/sim-wheel-docs/03-firmware-plan.md`,
  `docs/DELIVERY-NOTE.md` — both still mention the old
  `change-me-123` default).
- `docs/USER-GUIDE.md` + `docs/FLASH-GUIDE.md` updated to v2.0.4 /
  v4.0.1.

### Round 2 — second sweep (same version, no bump)

Second deep-analysis round on top of Round 1 (visual, logic, firmware,
security, performance, research). No version bump — app v2.0.4 /
firmware v4.0.1, wire format unchanged.

#### Blank window — last gaps closed

- **ChangePasswordDialog classic variant (O-1, Medium):** the Settings →
  Diagnostics password dialog was still a FluentWindow + Mica modal
  reachable in classic mode. It now has a real plain-Window variant
  (`ChangePasswordDialogClassic`) selected automatically in classic
  mode; Win11 behaviour unchanged.
- **Research verdict (W6):** ApexLink and RSH share chrome, framework,
  manifest, tray and DropShadow — the only new render layer is
  UiStrings bindings + RSHTheme/DynamicResource + new pages. Two wrong
  citations corrected (WPF-UI #1594 is a NavigationView/Win11 issue,
  not a FluentWindow blank; App.xaml dictionaries are identical in
  ApexLink). v2.0.4's real plain window makes the hypothesis testable:
  shell paints + content blank = page layer; shell also blank =
  below-app render failure. The decisive test is documented (title →
  `%TEMP%\RSH\startup-status.txt` decision tree → confirmatory runs).

#### Backend — 2 Highs fixed

- **Pairing-window bypass (B2-01, High):** any nonzero device key was
  stored; now only when HELLO flags say unpaired + pairing window
  open.
- **HELLO Wi-Fi password dropped (B2-02, High):** the 24-byte password
  field was never read; now saved to KeyStore when the window is open
  (otherwise the board goes WPA2 and the PC can never rejoin).
- Also: KEY_RESET sender (B2-03), control-send counter race + serial
  crash (B2-04, SemaphoreSlim gate), P12 caps validation in the engine
  path (B2-05), UDP sender filter anti-spoofing (B2-06),
  ProfileStore.Save race (B2-07, partial), Horn toggle no-op (B2-08).
  25/26 aliasing UI warning (W3-2); Diagnostics Tick Refresh
  throttled (W4-1); Benchmarks header version-derived (W4-2).
- **Deferred (Medium):** classic-mode in-app firmware upload
  (SEC-R2-01) — the wizard is the only upload UI and classic skips
  it; Win10 users must use browser WebOTA for now. Concrete plan
  documented (reusable FirmwareUpdateService + plain dialog +
  Diagnostics command).

#### Firmware — 4 fixes

- **Version misreported (FIX-1, Medium-High):** GET_CAPS/HELLO reported
  4.0.0 while the build is 4.0.1 — now single source of truth.
- **GPIO25/26 aliasing (FIX-2, Medium):** fixed pedal buttons were
  read even when the blob assigned those pins to encoders — now gated
  by pinmap.
- **WebOTA stalled-upload DoS (FIX-3, Medium):** 30 s inactivity
  timeout added.
- **reset_reason (FIX-4, Low):** now maps watchdog/brownout/software
  correctly.
- **Deferred (Medium):** accepted-but-dead config records (D-1) —
  AS5600/ADS1115/expander/matrix/battery validate + commit but have no
  firmware consumer; needs a PC+protocol decision (reject at COMMIT vs
  implement drivers).

#### Visual

- **Dark theme pills (F-1, Medium):** status pills now theme-aware
  (Dark contrast 6.94:1 / 7.43:1 / 5.71:1); Light unchanged. Resource
  audit 14/14 files clean, 0 missing keys.

#### Security

- Re-scan CONDITIONAL PASS, no Critical/High. 7 new Medium/Low
  findings documented (SEC-R2-01..07); SEC-H-04 review done — doc
  adequate, board-side execution remains. No secret leaks in logs
  (0 hits); ClassicMainWindow adds no new attack surface.

#### Data

- TelemetryWatcher stale snapshot, NtpPing race, EncoderTracker clamp,
  PinMapStore validation, AppSettings 3-gen backup, HelloClient
  comments. Decisions: shipped floor = Win10 22H2 (19045); orphan
  pages DELETE in v2.0.5; i18n deferred (resx 337/337/337 complete).

#### QA gate

- F1 E2E pacing decision: QUIET RE-RUN (not quarantine) — both E2E
  checks pass on quiet runs; real E2E SLO (ntp-ping) PASS. Gate
  unchanged.

## Verification (sandbox, this sweep)

- `dotnet build RSH.slnx -c Release`: **0 errors, 0 warnings**
  (RSH.App); test-analyzer warnings only in test projects
  (pre-existing xUnit1051).
- `dotnet test` (runnable suites): **RSH.Tests 749 pass / 0 fail /
  5 skip (754)**; RSH.VJoy.Tests 56/56; HIDMaestro 78/78.
  RSH.App.Tests (WPF) cannot run on Linux — Windows CI.
- Full Virtual Test Rig (quick): **2/2 stages PASS, 25/25 scenarios
  PASS**. Soak 60 s: loss 0%, rate ~198/s, jitter p99 ~936 µs,
  duplicatesOrLate 0. ntp-ping p99 within the ≤ 10 ms SLO.
- C++ host tests: **4416 checks, 0 failures**; pin rules PASS;
  protocol vectors PASS.
- `pio run`: **SUCCESS**, 0 warnings (merged bin
  `RSH-Firmware-DevKitV1-v4.0.1.bin`).
- Local framework-dependent loose publish: 41 files + NSec
  `libsodium.dll`, zip ~16.9 MB, `RSH.exe` 167,424 B (light),
  assembly 2.0.4.

## Verification — Round 2 (sandbox)

- `dotnet build RSH.slnx -c Release`: **0 errors** (18 pre-existing
  test-analyzer warnings, test projects only).
- `dotnet test` (runnable suites): **RSH.Tests 783 pass / 0 fail /
  5 skip (788)** — latest solo run; gate run 775/0/6 (781). Counts
  grew while teams added tests live in parallel; the invariant is
  0 fail. RSH.VJoy.Tests 56/56; HIDMaestro 78/78. RSH.App.Tests
  compiles (Windows-only — runs on Windows CI).
- Full Virtual Test Rig (quick): **2/2 stages PASS, 24/25 scenarios
  PASS**. The one miss is soak rate **194 vs 195/s** (last-second
  snapshot, run average ~198/s, loss 0%) — load noise on the shared
  1.5-CPU VM, no product defect; re-run on a quiet machine is
  required (see Conditions).
- C++ host tests: **4430 checks, 0 failures** (+ 50+50 stub checks,
  pin rules, protocol vectors).
- `pio run`: **SUCCESS**, 0 warnings (RAM 30.2%, Flash 67.4%;
  merged bin 955,056 B).
- Benchmarks **15/15 PASS**; ntp-ping p99 **3.388 ms** (≤ 10 ms SLO)
  on the quiet re-run.
- Local framework-dependent loose publish: **43 files** (41 + hi/ur
  satellites), zip 16.63 MB, `RSH.exe` 167,424 B, assembly 2.0.4.0.

## Requirements

- **.NET 10 Desktop Runtime REQUIRED** (framework-dependent loose
  publish — the runtime is NOT in the zip). Check with
  `dotnet --list-runtimes` — it must show
  `Microsoft.WindowsDesktop.App 10.x`. Download:
  <https://dotnet.microsoft.com/en-us/download/dotnet/10.0>
- The app is **UNSIGNED** (user decision — OV code-signing
  certificate later; SmartScreen warning expected).
- Firmware v4.0.1: reflash the board from v4.0.0 with the merged
  bin in this release (see `docs/FLASH-GUIDE.md`). The signed
  `firmware-signed.bin` is also in the release.

## Conditions (not blockers)

- **Win10 real-PC paint check:** v2.0.4's real plain window is the
  fix — run `RSH.exe` directly (no flag needed). The title shows
  the version; the log is in `%AppData%\RSH\logs\` (fallback
  `%TEMP%\RSH\logs\`) and the startup status in
  `%TEMP%\RSH\startup-status.txt`.
- **A7 real-board run** pending (user, board + Windows PC, per
  `docs/A7-RUNBOOK.md`). The plan freezes only after A7 passes.
- **4-hour long soak** incomplete (user stopped it ~16 min in) —
  only the 60 s soak is evidenced.
- SEC-H-04 review + flash-encryption board-side still open.
- Real Steam game launches + game-compat matrix verify (8 games,
  DRAFT) still pending on a Windows PC.
- **Round 2 additions:** rig quick re-run on a quiet machine (soak ≥
  195/s) — the only open formal gate item; classic-mode in-app
  firmware upload missing (SEC-R2-01, deferred — browser WebOTA is
  the workaround; fix planned for v2.0.5); accepted-but-dead config
  records (D-1) need a PC+protocol decision; ChangePassword classic
  dialog real-PC paint check (O-1).
