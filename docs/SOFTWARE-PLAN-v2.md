# RSH (Real Sim Hub) — PC Software Plan (v2.0 track)

Starting point: **RSH v1.2.2** (C# · .NET 10 · WPF + WPF-UI, vJoy output, 42 tests) — the version the owner tested and found working and reasonably optimized. The product is renamed **RSH — Real Sim Hub**.

**Universal by design (owner requirement):** the whole wheel is one ESP32 and the user attaches only what they want. The app has no fixed "rig" — it shows, tunes and maps **only the modules the user has added**. Power source (USB / battery / SMPS) is irrelevant to the app; battery display appears only if a battery module exists.

**Scope: everything requested is in scope** (per-module Tune, GPIO assignment, game launcher + per-game profiles, battery, LED strip/display, per-car profiles, expanders/encoders, latency tools). Order of delivery is by phase, not by dropping items.

> **Golden rule (owner):** v1.2.2 is fast and stable. **No change may make it slower, heavier, laggy or freeze-prone.** Section 0 turns this into measurable rules and release gates.
UI reference: the owner's **"RSH — Real Sim Hub" preview** (`rsh-ui-polished.html`: dark/light, accent palettes, Simple/Advanced mode, sidebar Dashboard · Devices · Tune · Profiles · Monitor · Settings).

---

## 0. Performance guardrails (non-negotiable)

### 0.1 What v1.2.2 already does right (must be preserved)

- The **engine runs on its own dedicated thread** (`RSH-Engine`, above-normal priority, drift-free 5 ms scheduler) — completely independent of the UI.
- Network receive/hello run on background tasks, never on the UI thread.
- The UI refreshes from **one 33 ms timer (~30 fps)** and **only the visible page ticks** (`IUpdatableView`) — hidden pages cost nothing.
- Hot path (packet parse + pipeline) is **allocation-free**; a benchmark tool (`tools/RSH.Benchmarks`) already measures ns/packet, allocations, loopback loss and soak memory growth.
- Framework-dependent "LITE" publish (~28 MB), workstation concurrent GC.

### 0.2 Rules for all new code

| # | Rule |
|---|---|
| P1 | **Engine thread is untouchable by features.** Game launcher, config sync, catalog scans, profile loads, LED telemetry output: none of them run on the engine thread or take a lock it needs. Hand-off only via lock-free snapshots (as profiles are applied today). |
| P2 | **No new per-tick work in the hot path** except what a module's tuning transform strictly needs. Every new transform (hysteresis, shifter pulse, button modes, layers) is a small struct operation with **0 allocations**, and has a benchmark. Modules that are OFF are skipped entirely (zero cost). |
| P3 | **One UI timer, throttled.** Keep ~30 fps for live values; graphs redraw only when the value changed visibly; pages not on screen do not update. No per-control timers, no per-control animations running at idle. |
| P4 | **UI thread does no I/O.** File scans, Steam/Epic detection, launching, config read/write, firmware calls, JSON load/save are all `async` off-thread; the UI gets results via one dispatcher post. Debounced saves (existing 400 ms pattern). |
| P5 | **Lists are virtualized** and graphs are lightweight drawings (single `StreamGeometry`/`DrawingVisual`, not hundreds of elements). Curve editor = one visual with a few handles. |
| P6 | **Game detection must be cheap:** catalog + library scan runs once at start (cached to disk, refreshed on demand/at idle), process watching uses one low-frequency poll (already 1.5 s for auto-switch) or a single WMI/ETW event subscription — never a tight loop. |
| P7 | **Idle must be idle:** when the window is minimised/tray, UI timer stops; only the engine + link run. Target near-zero UI CPU in tray. |
| P8 | **No heavy dependencies** (no embedded browser, no big UI frameworks, no reflection-heavy mappers). Any new NuGet package needs a size + startup justification. |
| P9 | **Memory is bounded:** fixed-size ring buffers for graphs/logs, no unbounded collections, soak test proves flat memory. |
| P10 | **Startup stays fast:** lazy-create pages on first visit (as now), no synchronous work before the first window paint. |

### 0.3 How "no regression" is guaranteed when nobody can measure on the owner's PC

The owner cannot run measurements and the development sandbox has no Windows/.NET runtime, so a numeric before/after baseline **cannot be collected**. (Decision: baseline measurement is *optional*, not a blocker — `docs/PERF-BASELINE.md` and `tools-perf/` stay available if it is ever done.) Performance is protected by construction instead:

1. **Do-not-touch zone.** The v1.2.2 hot path — `WheelEngine` loop, `UdpWheelSource`, `WheelPacket` parse/CRC, `ProcessingPipeline`, `OneEuroFilter`, `PedalRamp`, `VJoyOutput` — is **frozen**: only additive changes, no refactors, no reordering of work. Anything that must change there needs a written reason in the PR/commit and a micro-benchmark in the existing `RSH.Benchmarks`.
2. **New features are off-by-default and zero-cost when unused.** A module that is not added does not exist at runtime (no object, no tick work). The game launcher, config sync and LED/battery features have **no timer or thread running until the user opens/uses them** (watcher excepted — one 1.5 s poll, already in v1.2.2).
3. **Static review checklist on every change** (rules P1–P10 above): not on the engine thread, no allocation per tick, no I/O on the UI thread, no extra timers, virtualized lists, bounded buffers. Each phase's completion note states pass/fail per rule.
4. **Tests that run anywhere:** new allocation-free transforms get unit tests plus a micro-benchmark with a hard **0 B/tick assertion** (`GC.GetAllocatedBytesForCurrentThread`) — this check is deterministic and does not need a reference machine. Soak test asserts bounded memory (existing tool).
5. **Visible, built-in watchdog on the owner's PC:** hidden *Diagnostics → Performance* panel (engine tick avg/max, UI frame time, GC counts, working set, CPU). If anything ever feels slower, one look tells whether it is the engine, the UI or memory — and a **"Copy performance report"** button produces text to send back. No tools or commands needed.
6. **Kill-switches:** each heavy feature (game watcher, LED telemetry output, per-car detection, graph animations) has a Settings toggle so it can be turned off immediately if a user's PC struggles; animations also respect Windows "reduce animations".
7. **Staged delivery:** phases ship one at a time (S1 → S2 → S3 → S4); the owner tests each as he tested v1.2.2 and says "feels fine / lag". A phase that feels slower is fixed or reverted before the next starts — the owner's judgement is the gate, as it was for v1.2.2.

## 1. Requirements (from owner)

1. **Tune tab:** separate tuning *per module* — steering, throttle ("race" — assumed), brake, shifter, horn (+ clutch, handbrake, buttons, encoders), each with **advanced** features.
2. **Settings:** assign **GPIO pins from the software** (needs firmware plan F1/F2).
3. **Launch games from the software**, with a **separate profile/config per game**.
4. **UI/UX like the preview** — final design is made **only when the owner says so** (preview has bugs, section 4).
5. Industry-standard feel, optimized, universal/compatible (from the research round).

---

## 2. Architecture changes

```
RSH.Core            (keep) protocol, link, calibration, filters, profiles
  + Modules/             NEW  module model: type, enabled, pin binding, vJoy mapping, tuning schema
  + Profiles v2/         NEW  per-module tuning objects + game bindings + migration from v1
  + Device/              NEW  control-channel client (caps, get/set config, probe, commit/confirm)
  + Games/               NEW  game catalog, detection, launcher, process watcher
RSH.Output.*        (keep) IJoystickOutput; vJoy stays; second backend possible later
RSH.App             UI (WPF, decided — section 5)
```

- **Universal module system:** the user builds the rig by **adding modules** (Settings → Modules → *Add module*): Steering, Throttle, Brake, Clutch, Handbrake, Generic axis, Shifter (sequential / H-pattern), Horn / Button, Button matrix, Encoder, LED strip, Battery monitor, I²C expander. Nothing is mandatory except at least one input. **Hardware presets** pre-fill common builds (Minimal: steering + 2 pedals + 4 buttons · Full rig · Empty/custom) but the user can add/remove anything afterwards. Removing a module removes its Tune card, Dashboard bar and vJoy mapping instantly.
- **Module model:** every physical input is a `Module` with `Kind` (Steering, Throttle, Brake, Clutch, Handbrake, ShifterUp/Down, Horn, Button, Encoder), `Enabled`, `PinBinding` (from the board), `Output mapping` (vJoy axis/button), and a typed `Tuning` object. The Tune page, Dashboard bars and Monitor are all generated from the module list — an **OFF module disappears everywhere** (as the preview already intends).
- **Capabilities-driven pins:** pin pickers are built from the board's `GET_CAPS` response, never hard-coded. Works unchanged for other boards later.
- **Profile v2 (JSON, portable):** `{ modules: { steering: {...}, throttle: {...}, … }, buttons: [...], output: {...} }`. v1 profiles auto-migrate on load (backup file kept).

---

## 3. Feature plan

### 3.1 Tune — per-module tuning (Simple view = few sliders; Advanced = everything below)

| Module | Simple | Advanced |
|---|---|---|
| **Steering** | Rotation (90–1080°), response preset, set centre | Curve editor (draggable points, live in→out dot), sensor min/centre/max, centre deadzone, end deadzone, One-Euro smoothing (min-cutoff / beta), spike-reject strength, invert, centre-sensitivity, pot-linearity compensation, rotation presets per car class |
| **Throttle ("race")** | Calibrate (min/max), curve preset | Deadzone start/end, custom curve points, ramp/slew (up/down separately), smoothing, hysteresis, full-throttle threshold, invert |
| **Brake** | Calibrate, curve preset | As throttle + initial-travel deadzone, "hard-first" pressure curve, max-travel limit (ready for a load cell later), brake-light/threshold button output |
| **Clutch** | Calibrate | Bite-point marker, curve, deadzones, optional "button when pressed" |
| **Handbrake** | Calibrate | Analog or digital-threshold mode, latch/toggle option |
| **Shifter** | Sequential / H-pattern | Pulse length (ms — games need ≈30–60 ms), repeat-on-hold, lock-out between up/down, swap paddles, reverse-gear rule, gear display |
| **Horn / buttons** | Pick vJoy button | Mode: momentary · toggle · pulse · long-press (2nd action) · double-tap; debounce; shift/layer button (second function per button); optional keyboard-key output |
| **Encoders** | Mode | Pulse-as-two-buttons · axis · steps per detent · repeat |

Every card: live value + live graph, **Reset**, **Copy from profile**, **A/B compare** (hold to hear/feel the original), "unsaved" indicator, and **Save to wheel** only for hardware settings (curves live in the profile on the PC — see firmware plan §1).

### 3.2 Settings — Modules & pins

- **Add-module flow:** *Add module* → choose type → app proposes the best free pin(s) from the board's capability table → user can change them → wiring hint shown (divider/cap/pull-up where relevant) → Apply. Remove = one click (with undo until applied).
- **Pin budget bar:** "17 of 19 pins used — 2 free". When full, the app suggests a button matrix or an MCP23017 expander instead of failing.
- Table like the preview (module · on/off · type · pin), but with a **real pin picker**:
  - Lists only legal pins for that module type (ADC1 for analog, pull-up pins for buttons), illegal ones greyed **with the reason** ("ADC2 — unusable with Wi-Fi", "Strapping pin — may stop boot", "Used by Brake").
  - Duplicate/conflict detection live; **Apply** disabled while errors exist.
- **Apply flow:** Apply → board validates → app shows "Testing new wiring… Confirm?" (10 s countdown) → Confirm or auto-rollback. Never leaves the user with a dead board.
- **Wiring test:** per pin "Identify" streams the live level/ADC so the user can press the button / move the pedal and see it light up before assigning.
- Button & encoder assignment page (paddle −, paddle +, horn, ignition … → Button N) with press-to-test (already in preview).
- I²C expander section (ADS1115 / PCF8574 / MCP23017), Wi-Fi channel/port, firmware update, profile import/export, theme.
- "Read from wheel" and "Reset to factory map" buttons.

### 3.2b Power & battery

- Any power source is fine; the app never asks what powers the wheel.
- If a **Battery monitor** module is added: battery %/voltage tile on Dashboard + Devices, low-battery warning, chemistry selector (Li-ion / LiFePO4 / custom) or "external supply" mode (shows volts only). If not added: no tile anywhere.
- Wiring help card (divider ratio calculator, 3.3 V ADC limit, 3V3 reference, ripple capacitor note for SMPS supplies).

### 3.3 Games — library, launcher, per-game profiles

- **Catalog** (shipped JSON, editable): ACC, Assetto Corsa, AC Evo, iRacing, rFactor 2, Le Mans Ultimate, BeamNG, Automobilista 2, Project CARS, DiRT Rally 2.0, EA WRC, F1 (23–25), Euro/American Truck Simulator … each with defaults (suggested rotation, button hints, process name, Steam app id).
- **Detection:** Steam (registry + `libraryfolders.vdf` + `appmanifest_*.acf`), Epic (manifest folder), iRacing/other (registry/known paths), plus **manual "Add game"** (pick `.exe`) for anything else. Microsoft Store/Xbox titles: manual only in v1.
- **Launch:** Steam titles via `steam://rungameid/<id>`, others via exe + args + working dir. **"Launch with profile"** = pick that game's profile → make sure the wheel is connected and vJoy output is on → start game → watch the process → when it exits, restore the previous profile.
- **Per-game config:** profile (+ optional per-car later), rotation override, vJoy device id, launch args, pre-launch checks (wheel connected? vJoy ready?), "close companion apps"-style hooks kept out of v1.
- **Auto-switch** by foreground process already exists in v1.2.x and is reused (Profiles → "Per game").
- Note: the app only starts the game and changes its own settings — it does **not** inject into game processes, so anti-cheat systems have nothing to object to.

### 3.4 Output & compatibility

- Keep `IJoystickOutput`; **vJoy stays the default**, with a check/diagnostic for driver state (the vJoy fork is lightly maintained and ViGEmBus is retired — see research round).
- Latency instrumentation (end-to-end ms, jitter, loss) in Monitor — makes "optimized" measurable.
- Later option (separate decision): driver-free USB-HID dongle (ESP32-S3) as a second output path.

---

## 4. UI/UX direction

**Keep from the preview:** left sidebar with six pages, rounded cards, accent palette picker, Dark/Light, **Simple ↔ Advanced** switch, collapsible module cards in Tune, rig-status list with "Manage", press-to-test buttons grid, Get-set-up checklist, wizard + tour, connection pill, per-card "ON" state.

### 4.1 Bugs found in the preview (rendered at 1440 px, dark theme; light theme and phone width not yet checked)

| # | Bug | Fix in final |
|---|---|---|
| B1 | Literal `·`, `—`, `°`, `▾` text on screen (e.g. "139°", "Centre not set yet — …") — 32 escapes written as plain text | Proper characters / entities |
| B2 | Steering response graph is stretched to ~600 px tall and pushes the real controls off-screen | Fixed aspect ratio (≈ 16:9), max height |
| B3 | Pin dropdowns are fake: each shows 2 hard-coded options + an expander pin; no conflict detection; offers pins that are invalid for the job | Capability-driven picker (§3.2) |
| B4 | Preview's default pin map doesn't match the real wheel and has overlaps in meaning (e.g. brake on 33 where the firmware has a button) | Defaults come from the board |
| B5 | Inconsistent rows in Settings: "Shifter down", "Ignition" have no ON toggle while others do | One row design for all modules |
| B6 | "Preview controls (not part of the app)" box inside Monitor | Remove |
| B7 | Top bar is crowded (palette, theme, mode, profile, connection, help in one row) | Palette + theme → Settings; keep mode, profile, connection, help |
| B8 | Inter font loaded from Google Fonts — fails offline | Bundle the font |
| B9 | Dashboard/Monitor data is simulated | Bind to real link data |
| B10 | Keyboard focus/contrast not checked | Pass for focus rings, WCAG-AA contrast on all 4 palettes × 2 themes |

### 4.2 Design process — **gated**

No final visual design is produced until the owner says "go". Sequence once approved: (1) bug-free preview of Tune + Settings + Profiles with the new module/pin/game content, (2) owner review, (3) design tokens + component list frozen, (4) implementation.

---

## 5. Technology decision — **decided: stay on WPF, re-skin to match the preview**

| Option | For | Against |
|---|---|---|
| **A. Keep WPF, re-skin (chosen)** | Engine + UI stay in one process and one language; the tested, optimized v1.2.2 base is untouched; no extra runtime; small download; the app is already structured for it (single UI timer, per-page `IUpdatableView`) | Custom curve editor and themes need hand-built controls |
| B. WebView2 (HTML UI) | Preview becomes the UI almost as-is | An embedded Chromium engine runs extra processes and typically adds well over 100 MB RAM plus a slower start — directly against the "not heavy / no lag" rule; needs the WebView2 runtime on the PC; JS↔C# bridge adds a second thing that can stutter |
| C. Electron/Tauri | Cross-platform | Heavier again; vJoy is Windows-only anyway |

**Why this changed from my earlier recommendation:** the earlier lean toward WebView2 was about matching the preview faster. With your rule that v1.2.2's performance must not regress, the lower-risk choice is WPF. The preview is treated as a **visual spec** (layout, colours, spacing, behaviour), reproduced in WPF with a theme resource dictionary (4 accent palettes × dark/light as swappable brushes) and a small custom-control kit: Card, ModuleCard (collapsible), PillToggle, Segmented, Slider with live marker, CurveEditor, PinPicker, StatusPill. `Themes/Theme.xaml` from v1.2.2 is the starting point.

---

## 6. Phases

| Phase | Scope | Exit criteria |
|---|---|---|
| **S0 — Contract (DONE)** | Protocol v2 contract frozen (`PROTOCOL-V2.md`), pin table + reference validator, rename plan (§6.1), module list, performance-by-construction rules (§0.3) | Contract frozen by decision (owner delegated approval) |
| **S1 — UI shell** | WPF re-skin: sidebar/topbar, theme system (4 palettes × dark/light), control kit, Dashboard + Monitor on real data, bug list B1–B10 closed | Runs against Simulator and real board; **perf checklist passed (§0.3) + owner "feels fine"** |
| **S2 — Module model + Tune** | Module model, Profile v2 + v1 migration, Tune page with all per-module cards (Simple/Advanced) | Core unit tests for every tuning transform; each card's graph matches processing output; **0 alloc/tick benchmark; perf checklist + owner check** |
| **S3 — Pins & device config** | Control-channel client, caps-driven pin picker, apply/confirm/rollback, wiring test, expanders | Works with firmware F1+F2; bad config recovers automatically; perf checklist + owner check |
| **S4 — Games** | Catalog, detection, launcher, per-game profiles, restore-on-exit | Launch a Steam game / iRacing / manual exe with the right profile; restored on exit; **idle CPU unchanged with watcher running; perf checklist + owner check** |
| **S5 — Polish & release** | Light theme/accessibility, wizard update for new modules, docs, LITE publish (framework-dependent, ~30 MB), installer decision | Release checklist (§8) |
| **S6 — Extended features (in scope)** | LED strip + display page (needs firmware F5), battery/signal tiles, per-car profiles (needs game telemetry), encoders/expanders UI, latency tools; driver-free HID dongle output studied separately | Each sub-feature passes its own perf checklist + owner check |

### 6.1 Rename RSH → RSH (controlled, so nothing breaks)

| Layer | Plan |
|---|---|
| **User-visible** (window title, tray, wizard, docs, installer text, exe name `RSH.exe`) | Renamed in S1 |
| **User data** (`%AppData%\RSH` profiles/settings/logs, autostart registry value, single-instance mutex) | One-time **migration** from the old app-data folder to `%AppData%\RSH` on first run: copy, verify, keep the old folder untouched; autostart entry re-created under the new name |
| **Code namespaces / project names** (`RSH.Core` …) | Renamed mechanically in one dedicated commit with tests green before and after (no behaviour change allowed in that commit) |
| **Wire/firmware identifiers** (Wi-Fi SSID `RSH`, OTA hostname `rsh`, UDP magic) | **Renamed in Step 0** together with the app, so new app + new firmware stay in sync. Boards flashed with old firmware must be reflashed — the new app only joins the `RSH` network |
| Branding assets (icon `rsh.ico`, "RSH" mark from the preview) | Replaced in S1 |

Simulator (`tools/RSH.Simulator`) is extended to speak protocol v2 + the control channel, so S1–S3 can be built and tested **without** waiting for firmware.

---

## 7. Testing plan

- **Core unit tests** (grow from 42): one suite per tuning transform (curve, deadzone, ramp, hysteresis, One-Euro, shifter pulse, button modes), profile v1→v2 migration, pin-validator mirror of the firmware rules, protocol v2 golden vectors, control-channel framing/retry/HMAC.
- **Integration:** App ↔ Simulator (config apply, rollback, link loss), game launcher with fake exe + fake Steam manifests.
- **Manual on Windows (owner's PC):** look & feel, vJoy binding in a real game, Wi-Fi auto-connect, launch flow — these cannot be verified in my sandbox and will be listed as "pending owner check" as before.

## 8. Release checklist

- [ ] Build 0 errors / 0 warnings; all tests green
- [ ] v1 profiles load and migrate without loss
- [ ] Pin apply/rollback verified on the real board
- [ ] Every preview bug B1–B10 verified closed
- [ ] One game launched + auto-profile + restore verified on Windows
- [ ] Light + dark, all palettes, keyboard-only pass
- [ ] Docs: USER-GUIDE, HANDOVER, protocol doc updated

## 9. Risks

| Risk | Mitigation |
|---|---|
| vJoy driver aging / signing | Driver check in Diagnostics; keep `IJoystickOutput` swappable |
| Game detection misses installs | Manual add always available; catalog editable |
| Store/Xbox games can't be launched reliably | Documented manual-only in v1 |
| Scope too large for one release | Order is S1→S2→S3→S4; each phase shippable on its own |
| New features slow the app down | §0 rules + per-phase perf checklist + owner check; a failing phase is fixed or reverted |

## 10. Decisions

**All decided** (owner delegated the remaining choices to me):

| # | Decision |
|---|---|
| D1 | **WPF re-skin**, not WebView2 (performance rule, §5) |
| D2 | Product name **RSH — Real Sim Hub**; controlled rename (§6.1) |
| D3 | All requested features in scope; delivery by phase |
| D4 | "Race" = **throttle/accelerator** |
| D5 | Per-game profiles first, per-car in S6 |
| D6 | Broad game catalog + manual add |
| D7 | No installer in v2.0; first-run check for .NET runtime + vJoy |
| D8 | Universal modular rig on one ESP32; user adds only what they want |
| D9 | Power-agnostic; battery monitor is an optional module |
| D10 | Hardware presets: **Minimal · Full rig · Custom** |
| D11 | Protocol v2 contract **frozen as drafted** (`PROTOCOL-V2.md`) |
| D12 | Numeric perf baseline **not required** (cannot be measured); protection by construction + in-app Performance panel + staged owner checks (§0.3) |
| D13 | LED strip default **WS2812, up to 60 LEDs**; display support later, only if a concrete display is chosen |
| D14 | Feature kill-switches in Settings for every heavy feature |

Nothing is waiting on the owner. **No code is built until the owner says "start S1" (software) / "start F1" (firmware).** UI final design only on a separate "design go".
