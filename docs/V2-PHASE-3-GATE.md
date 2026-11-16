# RSH v2.0 — Phase 3 gate: shell + components — FINAL

- Date: 2026-11-16. Commit: (this commit). Branch: main, repo `dreamsofts34-creator/RSH`.
- Scope: `crates/rsh-ui/` (new binary crate), root `Cargo.toml`/`Cargo.lock`
  (workspace member + one-line tokio dep), this doc.
- Phase 2 API landed (`b27c1f3`): the shell is built against the real
  `AppApi`/`AppEvent` surface, live where the API covers it, simulated
  elsewhere. No competing API was invented.

## Build evidence

- `cargo check -p rsh-ui`: clean, 0 warnings.
- `cargo clippy -p rsh-ui --all-targets`: 0 warnings (verified twice
  consecutively on final code, 2026-11-16 ~15:00 PKT; `cargo check`
  also clean).
- `cargo fmt -p rsh-ui -- --check`: clean.
- Debug binary `target/debug/rsh-ui`: 377,529,960 B (final build with the
  min-width-920 fix).
- Window shown: 93–184 ms across 9 debug runs under Xvfb (quiet-ish host);
  772–1123 ms on the final re-render runs (loaded host); live-daemon run
  877 ms earlier the same day (loaded host).
- Full workspace test suite + Virtual Test Rig: **PENDING-QIET-HOST**
  (coordinator runs ONE batched quiet-host verification at integration;
  no full-suite/rig runs were done by this worker per the BOOST order).

## Design match (preview-html is the source of truth)

- Tokens exact (§5): bg #0D1118, side #10151D, panel #161C27, panel2 #1D2533,
  line #262F40, text #E9EDF4, mute #9AA5B8, faint #7C879B, accent #2F6BFF,
  ok #3DD68C, warn #F5B84A, bad #FF6B6B; Inter + JetBrains Mono embedded.
- Pixel census (screenshots under Xvfb, isolated display :98): sidebar
  border at x=183 (184px), top-bar border at y=55 (56px bar), window
  920×580. Nav: 3 entries (Overview / RSH Wheel + Steering/Pedals/Buttons/
  Extras / Settings), status card on every screen, AboutSlint widget in the
  About view (dark logo variant renders; "Version 1.18.1, slint.dev").
- Component library (§6): all 18 components exist as reusable Slint
  components in `ui/components.slint` and are used by the pages.

## What is verified

- Live IPC end-to-end: UI started against a real daemon
  (`--no-tray --no-connect`, test data dir) logs
  `RSH-UI daemon-live=true (live daemon data via rsh-ipc)`; the status card
  shows the REAL daemon state ("Disconnected / Wi-Fi · 192.168.4.1") and
  the snapshot shows real zeros, not sim data. `IpcApi` covers
  `connection()`, `snapshot()`, `output_enabled`/`set_output_enabled`,
  `connect()` (Reconnect button), `subscribe` + event drain logging.
  Disconnected Overview shows the §8.6 red banner + "Offline" pill +
  "Check needed" + warn checklist row (verified in a live screenshot).
- Sim fallback: `daemon-live=false`, "Simulated data" banner on Overview.
- Staging-worker findings addressed: (1) status-dot opacity bug — FIXED:
  pre-blended halo `#1D3E39`/`#402A33` + solid dot; census went from 0 to
  32 solid `#3DD68C` pixels at the dot. (2) `include_modules!()` — no
  change needed: `build.rs` compiles exactly ONE `.slint` file
  (`ui/main.slint`); the rest is imported. (3) `Palette.color-scheme` —
  not used anywhere in this crate (grep-verified); see open items.
- Phase 8 audit fixes (docs/V2-PHASE-8-AUDIT.md): FIX-1 window size —
  REPRODUCED first, then FIXED: the audit's 840px renders were real —
  Extras/Settings windows measured 839px on this worker's own renders
  too. Root cause: Slint opens the window at the content's preferred size
  clamped to [min-width, preferred-width], so narrow pages opened at the
  840px min-width. Fix: `min-width: 920px` in `main.slint` (the design
  width; the window can still be enlarged). An explicit
  `width: parent.width - 184px` binding on the content column was tried
  and REVERTED — Slint flags it as a binding loop ("may cause panic at
  runtime"). Verified after the fix: window right edge 919 on Overview,
  Extras, Settings AND Buttons; "Online" pill (29 green px) + "Configure"
  (1285 accent px) fully visible on Overview. FIX-2 primary `Btn` label
  is now white on accent (4.50:1, was Theme.text 3.83:1). FIX-3 `ButtonTile` label + "Not set"
  now use mute #9AA5B8 (was faint, worst 3.57:1). FIX-6 the dead Light
  theme option was REMOVED from Settings (Dark/Auto only; `theme` is
  still unread — light tokens do not exist yet). FIX-5 partial:
  `accessible-label` + `accessible-role: button` on `Btn`, `NavItem`,
  `SubItem` (the only controls whose label is in scope without plumbing).

## Known gaps (not silent drops)

1. **Content-column width (shell layout, Slint 1.18.1).** The content
   column takes the widest page's preferred width, not the full window:
   Extras/Settings render narrow (top-bar line ends x=732/704 instead of
   919) and Buttons overflows to ~768px, clipping the top-bar Save button
   to 39/52px visible ("Sav…"). `horizontal-stretch: 1` does not widen
   it; an explicit `width: parent.width - 184px` binding was tried and
   REVERTED — Slint flags it as a binding loop ("may cause panic at
   runtime"). Proper fix lands with the Phase 4/5 screens, whose
   preferred widths drive the column. Phase 4 must re-check the right
   edge on every page after wiring screens in.
2. **FIX-4 keyboard support — NOT DONE.** No `FocusScope`, no Up/Down
   navigation, no Enter=Save, no Esc handling anywhere. Lands with the
   drawer/popover work (Phase 5) or as a dedicated pass.
3. **FIX-5 remainder — NOT DONE.** `Switch` and every other TouchArea-only
   control still have no `accessible-label` (Switch needs a label plumbed
   through its use sites; the TouchArea-only tiles/chips need labels).
4. Overview "Firmware 4.0.1 · rsh.local · 200 updates/s" line and the
   "Pedals calibrated" / "Game controller ready" rows are still static
   preview copy — firmware/capabilities are not in the `AppApi` surface.
5. Frame data comes from trait `snapshot()` polling (20 Hz), not
   push-only `Snapshot` events — Phase 4 optimisation.
6. Pedals "100%" x-axis label sits tight at the card's right edge
   (inherited from the spike); Clutch "Bite point" hint shortened to fit.
7. Notification bell/toast UI lands Phase 6; tray popup/menu stay in the
   daemon (Phase 7); Pedals/Buttons editing Phase 4, Extras/Settings
   editing Phase 5.

## Open — Windows-only verification (cannot be measured on this sandbox)

- AboutSlint logo variant on a light-theme Windows PC (dark variant
  verified here; `Palette.color-scheme` must not be used to force it).
- Real tray click/menu, popup positioning, close-on-outside-click,
  per-monitor DPI, rounded tray-popup corners, head-to-head RSS vs the
  v1.0.2 Tauri baseline.
- Final visual check of the 920px window on the user's PC — local gates
  passed for v1.0.0/v1.0.1 and still failed there, so the width fix must
  be confirmed by the user's double-click, not by this gate.

## Final verification — DONE (2026-11-16, before commit)

- [x] `cargo clippy -p rsh-ui --all-targets`: 0 warnings (verified twice
      consecutively on final code). `cargo check`: clean. `cargo fmt`: clean.
- [x] Fresh debug build (377,529,960 B); re-rendered Overview, Buttons,
      Extras, Settings on the isolated display; census: window right edge
      919 on all four pages, top-bar line ends 919 (Overview/Buttons),
      "Online" pill + "Configure" fully visible on Overview, Settings
      Startup card fits (768 accent px in y480–580).
- [x] `git status` shows ONLY my paths in the commit: `crates/rsh-ui/`,
      `Cargo.lock`, `docs/V2-PHASE-3-GATE.md`. (Root `Cargo.toml` already
      lists `crates/rsh-ui` in HEAD `b27c1f3` — no diff.) No push/tag/release.

## BOOST accounting

- Affected-only checks during fixes (`cargo check/clippy -p rsh-ui`);
  no full-suite or rig runs (PENDING-QIET-HOST, one batched run by the
  coordinator). One screenshot pass for all 7 pages on an isolated X
  display (no re-runs for the shared-display contamination seen in the
  first pass). No test-round loops.
- Wasted: three failed scripted-edit rounds (bad string anchors /
  over-broad replaces) — all caught by asserts before any write; and two
  stale release-build attempts from mid-edit races (disregarded).
  Lesson recorded: verify anchors against the file before scripting
  multi-line replaces, and never build while editing the same files.
