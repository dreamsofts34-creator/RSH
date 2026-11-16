# RSH v2.0 — Phase 8 audit: DPI + accessibility + design match (§9/§11)

- Date: 2026-11-16, ~15:00 PKT (before the ~15:30 code freeze)
- Audited commit: `b27c1f3` (Phase 2 gate). `crates/rsh-ui/` is untracked
  work-in-progress from the Phase 3–7 workers (`.slint` mtimes 09:37–09:39,
  `target/debug/rsh-ui` built 09:40:44 — the binary matches the audited
  sources; the tree was still changing when this audit ran).
- Verdict: **NOT PASS — 8 FAIL, 10 PASS, 3 OPEN.** One FAIL dominates:
  the window opens at 840 px wide instead of 920 px, which clips the
  Overview page's right edge at every scale. Everything else that fails
  is small and specific (see FIX list).
- Method: read-only. No code, config or doc was edited except this file.
  Fresh headless renders of the existing `target/debug/rsh-ui` binary
  (no build): Xvfb `:99`, `SLINT_SCALE_FACTOR` 1 / 1.25 / 1.5,
  `--static --quit-after 8`, ffmpeg x11grab → `/tmp/p8-100.png`,
  `/tmp/p8-125.png`, `/tmp/p8-150.png` (ephemeral, not committed).
  Geometry measured by pixel census; contrast computed from the §5
  token values (WCAG relative luminance). Spike evidence:
  `docs/V2-PHASE-1-SPIKE.md` + `v2-spike/measurements/`. Staging
  evidence: `v2-screens/measurements/` + `screens/*-PORTING.md`.
  Contrast figures are exact ratios, not estimates.

## Counts

| Section | PASS | FAIL | OPEN |
|---|---|---|---|
| DPI | 2 | 1 | 0 |
| Accessibility (§9) | 0 | 5 | 1 |
| Design match (§11) | 8 | 2 | 2 |
| **Total (21 items)** | **10** | **8** | **3** |

## 1. DPI

| # | Item | Result | Evidence |
|---|---|---|---|
| D1 | Window opens 920×580 | **FAIL** | Fresh renders: content ends at x=839 at 100% (840 px), x=1048 at 125% (840×1.25), x=1258 at 150% (840×1.5). The window opens at `min-width` (main.slint:18), not `preferred-width: Theme.window-width` (920px, main.slint:16, tokens.slint:20). Same Xvfb, same flags: spike `overview.png` spans to x=918 and `dpi125.png` to x=1148, so this is app-side, not environment. Height is correct (580/725/870). |
| D2 | Sidebar 184px + top bar 56px at 125%/150% | **PASS** | Sidebar right border (LINE #262F40): x=183 @100%, 229 @125% (183×1.25=228.75), 274/275 @150% (274.5). Top-bar bottom border: y=55 @100%, 69 @125% (68.75), 83 @150% (82.5). All within 1 px of exact scaling. |
| D3 | Layout holds at 125%/150% (no new breakage) | **PASS** | `/tmp/p8-150.png` viewed: sidebar, cards, curve area, top-bar chip all scale cleanly; no overlap appears at 150% that is not already present at 100%. The only clipping at all scales is the D1/M10 right-edge clip, unchanged in kind when scaled. |

## 2. Accessibility (§9)

| # | Item | Result | Evidence |
|---|---|---|---|
| A1 | Tab order reaches all interactive controls | **FAIL** | Zero `FocusScope` and zero `KeyPressed`/key handlers in all 1,746 lines of `.slint` (`crates/rsh-ui/ui/`, `crates/rsh-daemon/ui/`) and in `crates/rsh-ui/src/main.rs` (grep `focus\|key\|accessible`: no hits). Every control is a `TouchArea` (components.slint:37, 82, 120, 140, 210–226, 249, 336, 363, 594, 724; shell.slint:76, 207) — mouse-only. Only the slider number box is natively focusable: raw `TextInput` (components.slint:297). Nav, switches, buttons, chips cannot be reached by Tab. |
| A2 | Up/Down moves in the navigator | **FAIL** | No key handling anywhere (see A1). `NavItem`/`SubItem` (components.slint:85–141, shell.slint:54–62) fire only on `clicked`. |
| A3 | Esc closes drawer/popover | **OPEN** | Nothing to close yet: Diagnostics drawer and Profile popover do not exist in the tree — `src/main.rs:350` "profile popover lands in Phase 5", `src/main.rs:388–389` "diagnostics drawer lands in Phase 5". No Esc handler exists either; add it with the drawer/popover (FIX-4). |
| A4 | Enter saves | **FAIL** | No key handling anywhere (see A1). Save is a mouse-only `Btn` (main.slint:157, shell.slint:425). |
| A5 | Text contrast 4.5:1 or better (§5 tokens) | **FAIL** | Computed from token values (tokens.slint:9–21). Passing: text #E9EDF4 on bg 16.11, on panel 14.55, on side 15.59; mute #9AA5B8 on bg 7.61, on panel 6.87, on panel2 6.19; faint #7C879B on bg 5.22, on panel 4.71; accent-text #8EB0FF on bg 8.83, on panel 7.97; ok 9.11, warn 9.63, bad 6.15 on panel. **Failing:** (a) primary-button label: Theme.text on accent #2F6BFF = **3.83:1** (Btn, components.slint:66–77 — every primary Save/Launch button); white on accent would be exactly 4.50:1. (b) `ButtonTile` label + "Not set" action in Theme.faint on panel2 #1D2533 = **4.25:1**, on acc-bg #152341 = 4.30:1, on pressed ok-bg #1B3635 = **3.57:1** (components.slint:587–592). |
| A6 | Every icon-only button has an aria-label | **FAIL** | Grep `aria\|accessible` over all `.slint`: **0 hits** in 1,746 lines. Slint has no `aria-label`; the equivalent `accessible-label` is likewise never set. The `Switch` (components.slint:20–38) is a text-less toggle whose name exists only as adjacent sibling text ("Send to game", StatusCard components.slint:148–176; Settings switches, shell.slint:576–581; AutoProfileChip pause switch, components.slint:252–270). Nav icons, the ProfileButton chevron (shell.slint:145) and the sidebar logo are decorative or inside labelled controls, so no strictly icon-only button exists — but no control carries any accessible name at all. |

## 3. Design match (§11)

| # | Item | Result | Evidence |
|---|---|---|---|
| M1 | Window 920×580, sidebar 184px, top bar 56px | **FAIL** | Sidebar/top bar PASS (D2). Window FAIL (D1): opens 840 px wide, not 920. Tokens are correct (tokens.slint:20–23) — the size is not honoured at open. |
| M2 | Colours/typography per §5 | **PASS** | Tokens exact: bg #0D1118, side #10151D, panel #161C27, panel2 #1D2533, line #262F40, accent #2F6BFF, Inter + JetBrains Mono (tokens.slint:9–33). Census re-run on the staging Settings render (`v2-screens/measurements/census-settings.py settings.png`): ALL PASS — bg/sidebar/line pixels, card geometry 342/342 px columns, accent/ok pixels, solid status + nav dots, diagnostics link. |
| M3 | Navigator: 3 entries + 4 sub-items | **PASS** | shell.slint:54–62: Overview, RSH Wheel (Steering/Pedals/Buttons/Extras), Settings. Matches preview Main.html. |
| M4 | Status card + Send-to-game on every screen | **PASS** | `StatusCard` is part of `Sidebar` (shell.slint:67–73), so it renders on every page (0–6); Send-to-game `Switch` inside it (components.slint:148–176). |
| M5 | Profile button + unsaved dot + Save (device pages) | **PASS** | main.slint:141–158: `SegmentedControl` + `ProfileButton` (unsaved dot: orange 8px disc, components.slint:245) + primary `Save` on pages 1–4. "Unsaved changes" text on Steering (shell.slint:277). |
| M6 | Basic/Advanced toggle (device pages) | **PASS** | main.slint:147–151, `SegmentedControl` (components.slint:189–230). |
| M7 | Slider + number box, preset chips, curve + live marker | **PASS** | `SliderRow` with 60px number box (`TextInput`, components.slint:280–345); preset chips Soft/Balanced/Sharp/Custom (shell.slint:264–267); `CurveGraph` grid + dashed line + accent curve + 12px live marker + crosshair (components.slint:366–442). Spike verified the marker moves with live input (docs/V2-PHASE-1-SPIKE.md §2). |
| M8 | Side-by-side screenshot compare (8 screens) | **OPEN** | Staging renders exist for Overview/Steering (spike), Pedals/Buttons/Settings (`v2-screens/measurements/*.png`); current-shell per-page renders were not produced in this audit (only Overview page 0 rendered fresh). Setup wizard and Tray popup do not exist yet (see M9). Re-render per page after FIX-1 and compare. |
| M9 | New screens (Extras, Diagnostics, Profile popover) per §8 | **OPEN** | Extras placeholder built with real §6 components (shell.slint:431–529). Diagnostics drawer and Profile popover not built (`src/main.rs:350,388–389` — Phase 5). NotificationCenter/bell also not built (only an `eprintln!` log hook, `src/main.rs:403–430`). |
| M10 | No clipped/overlapped English text | **FAIL** | Fresh `/tmp/p8-100.png` (Overview, viewed): right edge cut at the 840 px window — "Online" pill shows "Onli", "Firmware 4.0.1 · rsh.local · 200 updat…" truncated, "Configure" button shows "Configu…", iRacing "Locate" shows "Lo…". Same at 150% (`/tmp/p8-150.png`). Root cause is D1 (840 px width), not the strings: at 920 px these fit. Staging note: the ButtonTile 70px label slot overflows ~20px at the 840 px minimum window (`v2-screens/screens/PEDALS-BUTTONS-PORTING.md` §4.7) — that minimum is what the app currently opens at, so it bites now. No overlap found anywhere in the renders. |
| M11 | No Urdu/Hindi string or language switcher | **PASS** | Grep for Arabic/Devanagari ranges + `language\|locale\|ur.json\|hi.json\|switcher` over `crates/rsh-ui/ui/`, `src/`, `crates/rsh-daemon/ui/`: 0 hits. All visible strings English. |
| M12 | Disconnected / error / empty states present | **PASS** | `StatusCard` disconnected variant (halo-bad + bad dot, components.slint:148–176); "Simulated data — the daemon is not running yet" banner on Overview (shell.slint:97); `EmptyBanner` placeholders on Pedals/Buttons/Extras/Settings pages (shell.slint:301, 349, 431, 531). Full error-state screens (e.g. failed pairing) are Phase 6 work. |

## Notes (not counted above)

- AboutSlint licence widget is present on the About page (shell.slint:629–658), reachable from the sidebar footer — satisfies the Slint Royalty-free 2.0 attribution route chosen in Phase 1.
- Light theme: the Settings segmented control offers Dark/Light/Auto, but no light tokens exist (tokens.slint header: "Light theme is NOT designed yet"), Settings itself warns "Light theme needs design approval" (shell.slint:606), and `theme` is never read in `src/main.rs` — the Light option is currently a dead control (FIX-6). Deferred per plan §12 (approval needed), not a blocker for the dark-only release.
- Slint has no letter-spacing, so tracked-out labels (`DEVICE`, `APP`) render tighter than the preview — cosmetic, already recorded in the spike (docs/V2-PHASE-1-SPIKE.md §2).

## FIX list (for the integration step — one line each)

1. FIX-1: Make the window open at 920×580 (preferred-width), not the 840px min-width — then re-render all pages (main.slint:16–18).
2. FIX-2: Primary `Btn` label must be white on accent (4.50:1), not Theme.text (3.83:1) (components.slint:66–77).
3. FIX-3: `ButtonTile` label and "Not set" action must use mute #9AA5B8, not faint #7C879B (4.25/4.30/3.57:1 on panel2/acc-bg/ok-bg) (components.slint:591–592).
4. FIX-4: Add keyboard support: FocusScope on nav/buttons/switches/chips, Up/Down in the navigator, Enter = Save, Esc closes drawer/popover when built (no key handling exists anywhere today).
5. FIX-5: Add `accessible-label` to `Switch` and every other TouchArea-only control (currently 0 accessible/aria labels in 1,746 lines of .slint).
6. FIX-6: Wire the Light theme option to real light tokens or remove it — it is currently a dead control (shell.slint:606; `theme` unread in src/main.rs).
7. FIX-7: After FIX-1, re-render Overview/Steering/Pedals/Buttons/Settings at 920px and re-check the right edge (Configure, Online pill, iRacing Locate were clipped at 840px).
8. FIX-8: Re-run the Settings pixel census (`v2-screens/measurements/census-settings.py`) against the integrated shell's Settings render.

## Windows-only OPEN (cannot be verified on this Linux sandbox)

- Per-monitor DPI: move the window between monitors with different scale factors; change scale while running (spike §6 item 5, still open).
- Real tray click/menu and popup positioning near the tray icon (spike §6 items 2–4; popup is Phase 7).
- Tray-popup rounded corners on a real Windows desktop (square under Xvfb is a compositor artifact).
- Final visual check of the 920px window on the user's PC — local gates passed for v1.0.0/v1.0.1 and still failed there, so the width fix (FIX-1) must be confirmed by the user's double-click, not by this audit.
