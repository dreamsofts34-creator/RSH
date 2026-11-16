# RSH v2.0 — Phase 1: Slint spike

**Verdict: GO**

Slint reproduces the approved preview faithfully (Overview + Steering +
tray popup), with fast startup, acceptable RAM for a desktop app, working
DPI scaling, and a viable licence path. No hard blocker found. The items
that could not be verified on this sandbox are listed under OPEN with exact
Windows verification steps.

- Date: 2026-11-16
- Slint: **1.18.1** (latest stable, released 2026-09-21; verified via
  crates.io / docs.rs). `SystemTrayIcon` is built in since 1.17; `system-tray`
  is a default cargo feature.
- Toolchain: cargo/rustc 1.100.0
- Spike code: `~/workspace/projects/rsh/v2-spike/` (outside the repo;
  `Cargo.toml`, `build.rs`, `src/main.rs`, `ui/main.slint` ~1,250 lines,
  `assets/`, `run-spike.sh`, `measure.sh`, `README.md`)
- Repo HEAD at spike time: `dc84b5a` (Phase 0 gate)
- Environment: Linux sandbox, headless; Xvfb `:99` 1600×1200 + software
  rendering (no GPU), `dbus-run-session`. Fonts Inter + JetBrains Mono
  (OFL) embedded via `.slint` `import`.

## 1. Performance (spike binary, release, 31,412,832 B)

Measured on the sandbox via `measure.sh` (in-process `Instant` lines on
stderr + `/proc/<pid>/status` VmRSS of the real binary).

| Screen | Window shown | First tick | Idle RSS (live 20 Hz sim) |
|---|---|---|---|
| Overview (100%) | 59–100 ms | 60–102 ms | 178–184 MB |
| Steering (100%) | 100 ms | 102 ms | 181–182 MB |
| Tray popup (100%) | 79–92 ms | 80–94 ms | 189–191 MB |
| Steering @ 125% | 59 ms | 60 ms | 190 MB |
| Steering @ 150% | 68 ms | 69 ms | 197 MB |
| Static (no sim timer) | — | — | 171–182 MB |

- Wall-clock to first tick: 109–219 ms. One cold run after a VM restart
  took 1,842 ms (cold fontconfig cache); all warm runs are 59–100 ms.
- PSS ≈ 184.9 MB (RssAnon 91.5 MB, RssFile 95 MB); RSS is flat over a
  12 s run (no growth).
- CPU ≈ 75% of one core during the live 20 Hz sim under Xvfb — a sandbox
  artifact (software rendering, no GPU). A static app is near 0%.
- **RAM is much higher than "Slint is tiny" expectations** (~180 MB vs the
  single-digit MB often quoted for embedded Slint). Recorded as a finding,
  not a blocker: Linux/X11/fontconfig overhead differs on Windows, and
  ~180 MB is acceptable for a desktop companion app. The Windows
  head-to-head vs the v1.0.2 Tauri baseline is still pending — Tauri GUI
  cannot run on this Linux sandbox, so no baseline number exists here.
- Screenshots: `v2-spike/measurements/{overview,steering,tray,dpi125,dpi150,static}.png`.

## 2. Design match (preview = single source of truth)

Compared side-by-side with `preview-html/Main.html`, `Wheel.html`,
`Tray.html`, `TrayMenu.html`. Tokens exact per REDESIGN-PLAN §5
(window 920×580, sidebar 184px, top bar 56px, bg `#0D1118`, panel
`#161C27`, accent `#2F6BFF`, Inter + JetBrains Mono, English only).

**Overview** — matches: sidebar (Overview / RSH Wheel + Steering/Pedals/
Buttons / Settings, status card with Send-to-game switch), top bar
(Overview title left, auto-profile chip right), device box (rotating wheel
+52°, live Throttle/Brake/Clutch bars + %), games tiles (Launch/Stop/Locate
at right edge), Ready-to-race checklist (hints right-aligned: Wi-Fi,
12 Nov, HIDMaestro), connection-quality card (4 ms / 0% / 1 ms).

**Steering** — matches: response-curve card drawn with Slint `Path`
(grid + dashed straight line + accent power curve + dashed crosshair +
live marker), readout `Input 78% / Output 70%`, Feel card (Soft/Balanced/
Sharp/Custom, deadzone 2%, curve 1.40, Reset / Unsaved changes), Steering
centre card (Set pill, Set centre button). Marker moves with the simulated
input; the curve recomputes from the exponent.

**Tray popup (340×440)** — matches `Tray.html`: header (RSH Wheel,
Connected · 4 ms, switch), live box (wheel + 3 pedal bars with fills),
games list (Assetto Corsa / ACC / iRacing rows), footer buttons.

### Bug found & fixed during the spike (layout alignment)

Non-stretch `alignment` on a Slint layout prevents stretch children from
growing (docs: elements only grow beyond preferred size when the layout
alignment is `stretch`, the default). Symptoms, confirmed by pixel census
of the screenshots:

- Tray HBar tracks/fills did not paint (0 fill px; track ≈ 4 px wide).
- Top-bar title + chip were centred as a group instead of left/right.
- Checklist hints, game-tile buttons, sidebar nav dot, and the curve
  legend all collapsed to the left of their rows.

Fix: removed `alignment: center` from 10 layout lines and
`alignment: start` from 4 (NavItem, CheckRow, GameTile, curve readout
row); added explicit `horizontal-stretch: 0` to 6 fixed-size siblings
that would otherwise share the freed space. Pure-centering layouts with
no stretch child kept their alignment. Verified after the fix:

- Tray: track 1,196 px, throttle fill 456 px, brake 256 px, clutch 83 px.
- Hints end at x=570 (card edge), nav dot at x=157–162 (right edge),
  title glyphs at x=205–279, chip border at x=593–905.
- Also fixed in the same pass: response-curve number box showed `1.4`
  (Rust `"{:.1}"`); now `1.40` as in the preview.

### Remaining deviations (cosmetic / spike-only)

- Slint has no letter-spacing, so tracked-out labels (e.g. `DEVICE`,
  `APP`) render slightly tighter than the preview. Minor.
- Tray popup has square corners under Xvfb (no compositor); rounded
  corners are expected on a real Windows desktop — unproven here.
- Pedals / Buttons nav items are inert in the spike (no pages built for
  them); Settings page is not built. Sliders/number boxes are display-only
  (no engine behind them); data is a sine-sweep simulation.
- Slint `Path` has no dash support — dashed lines are generated as path
  segments in Rust. Works, but the code is more verbose.

## 3. Tray

- `SystemTrayIcon` (Slint 1.18.1) compiles and registers; the right-click
  menu structure matches `TrayMenu.html` (Open RSH, Send to game ✓,
  Profile ›, Launch game ›, Restart wheel, Settings, Quit RSH) with a
  nested `Menu` as a sibling with `title:`.
- **Linux sandbox: no tray host.** Under Xvfb there is no StatusNotifier
  host, so creation fails at runtime:
  `Failed to create system tray icon: Failed to spawn ksni tray: failed to
  register to the StatusNotifierWatcher: org.freedesktop.DBus.Error.ServiceUnknown`
  — expected here. The icon/menu click behaviour is therefore
  **untestable on this sandbox**; it needs a desktop with a StatusNotifier
  host (Windows: the notification area).
- The tray **popup window itself works**: shown at the CLI position with
  the correct 340×440 layout.
- **Popup positioning near the tray icon is NOT possible via the Slint
  API** — no tray-icon geometry is exposed (confirmed from the docs and
  the generated API). The spike places the popup at a CLI/default
  bottom-right position. Windows positioning via
  `Shell_NotifyIconGetRect` is an OPEN Phase-7 item.
- Popup close-on-outside-click is also unimplemented (no focus-out event
  exposed) — OPEN.
- API gotcha: `SystemTrayIcon`'s built-in `clicked` is not exposed as
  `on_clicked` in generated Rust 1.18.1; forward it in `.slint`
  (`clicked => { tray-clicked(); }`) and handle `on_tray_clicked` in Rust.

## 4. DPI

`SLINT_SCALE_FACTOR` env var (confirmed in the winit backend source).
Window physical size scales exactly: 125% → 1150×725, 150% → 1380×870;
layout holds at both scales (verified visually at 150%: sidebar, top bar,
curve card + legend, Feel card, Steering centre card all scale cleanly).
RSS grows with scale (190 MB @ 125%, 197 MB @ 150%) — more pixels to
render. Windows per-monitor DPI behaviour (mixed-DPI, scale change while
running) is untested here — OPEN.

## 5. Licence — verdict: VIABLE under Slint Royalty-free 2.0

- Slint is tri-licensed: **GPLv3 | Royalty-free 2.0 | commercial**.
  Choosing Royalty-free 2.0 avoids GPLv3 entirely — **GPLv3 is forbidden
  for RSH (MIT) and is not used**.
- Royalty-free 2.0 is free for proprietary desktop/mobile/web apps and
  requires attribution via EITHER the `AboutSlint` widget in an About
  screen/dialog accessible from the top-level menu (or splash screen) OR
  the "Made with Slint" badge on the public download page. Plan: add the
  `AboutSlint` widget to the Settings → About screen in Phase 2.
- It excludes Embedded Systems and forbids standalone distribution and
  re-exposing Slint APIs. Neither bites RSH: Slint is used only in the
  desktop app, not the ESP32 firmware; the app is distributed as a whole,
  and no Slint API is re-exposed.
- An MIT-licensed desktop app may use Slint under Royalty-free 2.0.
- Sources (official):
  - Licence text: https://raw.githubusercontent.com/slint-ui/slint/master/LICENSES/LicenseRef-Slint-Royalty-free-2.0.md
  - Slint 1.17 release (tray + licence notes): https://slint.dev/blog/slint-1.17-released
  - SystemTrayIcon reference: https://snapshots.slint.dev/master/docs/slint/reference/window/systemtrayicon/
  - Fonts: https://docs.slint.dev/latest/docs/slint/guide/development/fonts/

## 6. OPEN items (Windows verification needed)

1. **Head-to-head vs v1.0.2 Tauri baseline** — idle RSS + startup on the
   same Windows PC (double-click the installed app, read Task Manager /
   `%AppData%\RSH\logs` timing). Tauri GUI cannot run on this sandbox.
2. **Real tray click/menu test** — install on Windows, left-click the tray
   icon (popup opens), right-click (menu opens, items fire), quit from the
   menu. Also verify the popup does not leave a console/terminal behind.
3. **Popup positioning near the tray icon** — implement
   `Shell_NotifyIconGetRect` in Phase 7; verify the popup appears next to
   the icon on single- and multi-monitor setups.
4. **Popup close-on-outside-click** — design the Phase-7 behaviour and
   verify on Windows.
5. **Per-monitor DPI** — move the window between monitors with different
   scale factors; change scale while running; confirm no layout breakage.
6. **Rounded tray-popup corners** — confirm on a real Windows desktop
   (square under Xvfb is a compositor artifact).

## 7. Verification scope (BOOST)

Per the parent BOOST order, redundant re-runs were cut: after each small
fix only the affected screens were re-measured (tray + overview after the
alignment fix; steering once after the `1.40` format fix). No full
re-measurement suite was run after any fix, and the DPI runs were done
once. The phase gate itself (build → test → fix → gate) was not cut.

## 8. Sandbox workaround (not part of the product)

The sandbox lacks `libxkbcommon-x11.so.0`, so winit panics at startup.
`apt install libxkbcommon-x11-0` fixed it, but VM restarts wipe `/usr`;
the spike scripts therefore set `LD_LIBRARY_PATH` to a minimal C stub in
`v2-spike/dev-only/` (keyboard input only; rendering unaffected). All
headline numbers above were measured with the real library; post-restart
re-runs with the stub land in the same ranges. The stub must never ship.
