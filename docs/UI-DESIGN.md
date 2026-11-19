# RSH v2.0 — UI Design

**Status:** approved design, implemented. **App:** Slint UI + daemon ·
**Window:** 920×580 · **Theme:** dark (approved) · **Language:** English only

> **Single source of truth:** the approved preview (`preview/` and
> `preview-html/` in the redesign plan, outside this repo). This
> document describes that preview. Layout, colors, spacing,
> components, navigation and screen structure match the preview
> same-to-same — no design change without user approval. Where the
> shipped app deviates from the original plan, the deviation is
> marked below.

## 1. Principles

1. **Compact:** one screen per job, minimal scrolling. Window
   920×580 default and minimum (never smaller).
2. **Only 3 nav entries:** Overview, RSH Wheel (Steering, Pedals,
   Buttons, Extras), Settings. No extra tabs.
3. **Beginner-friendly words:** "Send to game" (output), "Pedal
   up / Pedal down" (min/max), "Lost data" (packet loss), "Game
   controller" (backend).
4. **Basic first, Advanced later:** advanced controls hide behind a
   Basic/Advanced toggle.
5. **Always show what is happening:** connection status and the
   Send-to-game switch on every screen (sidebar status card).
6. **No auto-save, clear unsaved indicator:** orange dot on the
   profile button + Save button.
7. **Live feedback:** every control shows its live value (curve
   marker, bars, pressed-button highlight).

## 2. Layout system

- Window **920×580** (default and minimum), resizable, DPI-aware.
- Sidebar **184px** (left). Top bar **56px**. Content padding
  16px 20px. Gap between cards 12px.
- Cards: radius 12px, border 1px, padding 14px 16px.
- Content grids: Overview = stacked rows; Steering = 1fr + 296px;
  Pedals = 3 equal columns; Buttons = 1fr + 268px; Settings =
  2 equal columns.

### Colors (dark theme, approved)

| Token | Value | Use |
|---|---|---|
| bg | `#0D1118` | Window background |
| side | `#10151D` | Sidebar |
| panel | `#161C27` | Cards |
| panel2 | `#1D2533` | Inputs, buttons, tiles |
| line | `#262F40` | Borders |
| text | `#E9EDF4` | Primary text |
| mute | `#9AA5B8` | Secondary text |
| faint | `#7C879B` | Labels |
| accent | `#2F6BFF` | Primary button, active state |
| accent-text | `#8EB0FF` | Links, accent on dark |
| ok | `#3DD68C` | Connected, good |
| warn | `#F5B84A` | Unsaved, running |
| bad | `#FF6B6B` | Brake bar, errors |

Light theme is **not designed yet** — every dark token needs a
light version, and it ships only after approval.

### Typography

- Text: **Inter** (400/500/600/700), base 13px.
- Numbers (angle, %, mV, ms): **JetBrains Mono**, tabular numbers.
- Page title 17px/600, card title 12.5px/600, small labels
  11–12px, section label 10.5px uppercase.
- UI language: **English only**. Copy is short, clear and
  professional — no slang, no technical jargon where a simple
  word works.

## 3. Navigation

```
[Logo] RSH · Real Sim Hub
Overview
DEVICE
  RSH Wheel  ●
     Steering
     Pedals
     Buttons
     Extras
APP
  Settings
-----------------
[Status card]  ● Connected · Wi-Fi · 4 ms
               Send to game   [switch]
```

## 4. Components

| Component | Spec |
|---|---|
| Nav item | Height 36px, icon 17px + label. Active: accent tint + white text. Sub item: 30px, left padding 38px |
| Status card | Sidebar bottom: dot + "Connected / Wi-Fi · 4 ms", "Send to game" switch below |
| Switch | 34×20 (top bar/cards), 40×22 (tray). On = accent |
| Segmented control | Basic/Advanced, Dark/Light/Auto, Wi-Fi/USB |
| Profile button | Top-right: name + chevron. Orange dot when unsaved. Primary "Save" beside it |
| Auto-profile chip | Top bar: "Auto profile · Game → Profile" + pause switch |
| Card | Title (12.5px) + optional right-side hint |
| Slider + number box | Range slider (accent) + 54–60px mono number box, type a value directly |
| Preset chips | 3–4 chips (Soft / Balanced / Sharp / Custom). Active = accent border + tint |
| Curve graph | Grid + dashed straight line + accent curve + live marker + readout "Input 42% / Output 29%" |
| Pedal bars | Vertical 16px bars (Overview, Pedals), horizontal 6–8px (tray). Throttle ok, brake bad, clutch accent |
| Range block | "Pedal up / Pedal down" values + "Set up / Set down" buttons |
| Button tile | "BUTTON n" + action. States: normal, selected (accent), live pressed (green), not set (faint) |
| Game tile | Icon square + name + profile + status pill + Launch/Stop |
| Checklist row | Green tick circle + text + right hint |
| Pill | Ok / warn / neutral / accent |
| Radio option card | Game controller choice (HIDMaestro / vJoy) |
| Empty / error banner | Inside a card, warn/bad colour, one line + one action button |

## 5. Screens

### 5.1 Overview

1. **Device box:** wheel graphic (live rotation) + steering angle,
   3 pedal bars with %, device name, "Online" pill, firmware,
   address, update rate; profile button + "Configure".
2. **Games:** tiles with profile, status (Ready / Running /
   Not found), Launch / Stop. Games are added **manually**
   (name + exe/shortcut) — there is no auto-detection (see §8).
3. **Ready to race** checklist (wheel connected, pedals
   calibrated, game controller ready) + **Connection quality**
   (Delay, Lost data, Jitter). Clicking the quality card opens
   the Diagnostics drawer.
4. Top bar: title + auto-profile chip.

### 5.2 Steering

- Top bar: Basic/Advanced, profile button (unsaved dot), Save.
- Left: **Response curve** card with live marker.
- Right: **Feel** card (presets, Centre deadzone, Response curve
  sliders, Reset to defaults, "Unsaved changes") + **Steering
  centre** card (Set centre).
- Advanced mode: min/max mV, centre mV, raw live value, curve
  exponent number box.

### 5.3 Pedals

- 3 cards: Throttle, Brake, Clutch. Each: live bar + mini curve,
  Pedal up/down (mV) + Set buttons, feel chips, Deadzone slider.
  Pedal graphs update live at 20 Hz, like steering.
- Clutch: extra **Bite point** slider.
- Calibration rule: to "Set down", press the pedal fully, then
  press the button.

### 5.4 Buttons

- Left: 12 button tiles (live pressed highlight), **Rotary knobs**
  card below (Knob 1, Knob 2: left/right actions).
- Right: edit panel: "Press any button on the wheel" (learn),
  action chips (Gear 1–6, Shift up/down, Horn, Handbrake,
  Not set), Cancel/Save, 30 second rollback note (the firmware
  confirm timeout is 30 s).

### 5.5 Settings

- Left column: Connection (Wi-Fi/USB, address, Pair), Game
  controller (HIDMaestro / vJoy), Startup (Start with Windows,
  Keep running in tray).
- Right column: Profiles (list, Active, Duplicate, Use, New),
  Appearance (Theme), Backup and updates (firmware, app,
  backup/restore). No language selector (English only).
- Small link at the bottom: **Open diagnostics**.

### 5.6 First-time setup (5 steps)

Left: logo, "Set up your wheel", 5 steps (Connect, Drivers, Pair,
Calibrate, Games). Right: step content.

- **Connect:** Wi-Fi/USB + address. Next unlocks when the board
  is reachable.
- **Drivers:** vJoy controller check/install, HIDMaestro fallback.
- **Pair:** 60 second countdown ring + 3 simple steps + live
  status + "Wheel not found? Use a USB cable".
- **Calibrate:** centre + pedals, "Set" buttons.
- **Games:** add games, choose game controller.
- Hidden (auto/advanced): password/key, pin map, firmware. They
  appear only when needed (firmware mismatch, pins edit in
  Advanced).
- "Skip for now" is always present.

### 5.7 Tray popup (left click)

340×440. Header (name, status, Send to game switch), live wheel +
3 horizontal bars, profile selector, 3 games (Launch), footer
"Open RSH" / "Restart wheel". Opens near the tray; closes on
outside click. Stays visible at least 5 s.

### 5.8 Tray menu (right click)

240px: status header, Open RSH, Send to game (tick),
Profile ›, Launch game ›, Restart wheel, Settings, Quit RSH.

## 6. Other screens (same shell, same components)

- **Extras** (RSH Wheel → Extras): one page, 2×2 grid of 4 cards
  (Handbrake, Shifter, Horn, LEDs). Each card header has an
  on/off switch; an off module greys its card ("Off, turn on
  to edit").
- **Diagnostics drawer:** right-side drawer (420px) from Overview
  "Connection quality" or Settings. Live charts (Steering,
  Pedals, 10 s), packet stats (delay, jitter p99, lost data, loop
  rate, TX failures, sequence gaps), event log, live button grid
  and encoders. Close button + Esc.
- **Profile popover:** click the profile button. Presets
  (read-only, duplicable) and "My profiles"; each row: name,
  linked game, Active tick. New / Duplicate / Rename / Delete /
  "Link to game". Switching with unsaved changes asks
  "Save / Discard" first.
- **Notifications:** bell icon beside the sidebar status card,
  count badge. List (connection lost, firmware mismatch, backup
  done). Toasts top-middle, 4 s.

## 7. States (every screen)

- **Disconnected:** status card red/grey, Overview banner "Wheel
  not connected" + Reconnect + "Run setup".
- **Pairing needed:** banner + Pair button.
- **Game controller missing:** warn pill in Settings, warn row in
  the Overview checklist + "Fix".
- **Loading:** skeleton rows, never bare "Loading…" text alone.
- **Error:** one line inside the card + Retry.
- **Rollback:** a failed Buttons/config save offers "Roll back".

## 8. Deviations from the original plan (approved, shipped)

- **Games are manual.** The original plan found games by
  scanning (Steam/iRacing auto-detection). v2.0.4 removed the
  scanner entirely: the user adds games by name + exe/shortcut
  and launches them locally. No "Locate" button, no scanning.
- **Setup wizard is 5 steps.** The plan had 4; the Drivers step
  was added for vJoy. Step order is Connect → Drivers → Pair →
  Calibrate → Games.
- **Buttons confirm timeout is 30 s** (firmware
  `RSH_CFG_CONFIRM_TIMEOUT_MS 30000`), not the 10 s in the
  plan's text.
- **Window never below 920×580.** The plan's 840×540 minimum
  clipped page right edges.
- **Pedal graphs are live at 20 Hz**, like steering.
- **Light theme deferred.** Dark is the approved theme; light
  tokens are not designed yet and ship only after approval.

## 9. Language, copy, accessibility

- English only. No Urdu/Hindi strings, no language switcher.
- Copy: short sentences, active voice, the exact action on the
  button ("Save", "Set centre", "Launch"); errors say what
  happened and what to do.
- Keyboard: Tab order, Up/Down in the navigator, Esc closes
  drawer/popover, Enter saves.
- Contrast 4.5:1 or better. Every icon-only button has an
  aria-label. Real `<button>` / `<input>` elements.
- DPI scaling tested at 125% / 150%.

## 10. Design match checklist (before sign-off)

- [ ] Window 920×580, sidebar 184px, top bar 56px
- [ ] Colors/typography as §2
- [ ] Navigator: only 3 entries + 4 sub-items
- [ ] Status card and Send-to-game on every screen
- [ ] Profile button + unsaved dot + Save (device pages)
- [ ] Basic/Advanced toggle (device pages)
- [ ] Slider + number box, preset chips, curve + live marker
- [ ] Side-by-side screenshot compare with the preview
      (Overview, Steering, Pedals, Buttons, Settings, Setup,
      Tray popup, Tray menu)
- [ ] New screens (Extras, Diagnostics, Profile popover) in the
      same style
- [ ] No clipped or overlapping English text in any card/button
- [ ] No Urdu/Hindi string or language switcher left
- [ ] Disconnected / error / empty states present
