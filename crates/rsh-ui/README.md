# rsh-ui — RSH v2.0 app shell + component library

The main RSH window (Slint 1.18.1, dark theme, 920×580): 184px sidebar,
56px top bar, status card on every screen, and the 18 reusable components
from REDESIGN-PLAN §6. Placeholder pages host the real screens that land
in Phase 4 (Pedals, Buttons) and Phase 5 (Extras, Settings).

## Layout

- `ui/main.slint` — `MainWindow`: sidebar + top bar + page host. Page ids:
  0 Overview, 1 Steering, 2 Pedals, 3 Buttons, 4 Extras, 5 Settings, 6 About.
- `ui/shell.slint` — `Sidebar` + the 7 pages (Overview/Steering rebuilt on
  the component library; Pedals/Buttons/Extras/Settings are placeholders).
- `ui/components.slint` — the §6 library: Card, Switch, Pill, Btn, NavItem,
  SubItem, StatusCard, SegmentedControl, ProfileButton, AutoProfileChip,
  SliderRow, PresetChip, CurveGraph, PedalBar, HBar, RangeBlock, ButtonTile,
  GameTile, CheckRow, RadioOptionCard, EmptyBanner, WheelGraphic.
- `ui/tokens.slint` — REDESIGN-PLAN §5 tokens + pre-blended derived fills.
- `src/main.rs` — frame loop (20 Hz), tuning state, callbacks.
- `src/ipc_api.rs` — `IpcApi`: the live `AppApi` over `rsh-ipc`.
- `src/sim_api.rs` — `SimApi`: simulated data fallback (banner shown).
- `src/live_probe.rs` — `try_live_api()`: daemon socket probe at startup.
- `src/curve.rs` — steering-curve path math for `CurveGraph`.

## Run

```sh
cargo run -p rsh-ui -- [--page overview|steering|pedals|buttons|extras|settings|about] [--static] [--quit-after SECONDS]
```

Data: if the daemon IPC socket is reachable the UI uses live daemon data
(`RSH-UI daemon-live=true` on stderr); otherwise it falls back to simulated
data and shows a "Simulated data" banner. `--data-dir DIR` overrides the
data dir (test-only: points the IPC socket probe at DIR instead of the real
`%APPDATA%/RSH`-equivalent dir).

## Notes

- Single Slint root: `build.rs` compiles only `ui/main.slint` (the rest is
  imported), so `slint::include_modules!()` sees exactly one file.
- `Palette.color-scheme` is NOT used (does not parse in Slint 1.18.1);
  the AboutSlint logo variant on a light-theme Windows PC is an open
  gate item (see `docs/V2-PHASE-3-GATE.md`).
- The status-dot halo is a pre-blended solid (`Theme.halo-ok`/`halo-bad`),
  never an `opacity` rect — opacity dims the dot itself (spike bug).
- Layout rule (spike + Phase 3): a non-stretch `alignment` on a layout
  stops its children stretching in BOTH axes; prefer explicit
  `vertical-stretch: 0` + a trailing spacer Rectangle for top-aligned pages.
