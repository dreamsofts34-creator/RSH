# UI LOCKED — RSH App (UI-12, Phase E Workstream 9)

**Date:** 2026-11-13 · **Status: LOCKED.** Is ke baad koi UI change sirf naya version
(preview parity checklist §4 + a11y/i18n — `RSH-UI-PLAN.md` §2 UI-12 / §4).

## Parity checklist (§4) — result

| # | Item | Result | Evidence |
|---|---|---|---|
| 1 | 8 screens, wahi naam, wahi order | ✅ PASS | `MainWindow.xaml` nav `Tag`: Dashboard, Tuning, Setup, Buttons, Output, Profiles, Monitor, Settings — `_pages` dict mein har entry asal page hai (koi placeholder nahi) |
| 2 | Colours §0.1 (accent #6C4CF1, light bg #F5F3FF) | ✅ PASS | Tokens `Themes/RSHTheme.xaml` (`RSHAccentColor` #6C4CF1) + palettes (light bg #F5F3FF, dark bg #0B0E13); XAML mein hard-coded `#6C4CF1` = 0 (sirf token files); `#FFB000` = 0 |
| 3 | Dashboard: wheel + pedal bars + game launcher (4 games) | ✅ PASS (1 note) | `DashboardPage.xaml` (live wheel, Accel/Brake bars — clutch bar nahi: protocol v2.1 mein clutch channel hi nahi, fake bar kabhi nahi); `GameLauncher.cs`: Assetto Corsa, ACC, iRacing, Dirt Rally 2.0. **Note:** page heading "Home" tha — "Dashboard" kar diya (nav naam se match) |
| 4 | Tuning: 10 cards, tap → ek panel, live curves | ✅ PASS | `CalibrationTuning.cs:429-438`: 10 modules (Steering, Accelerator, Brake, Clutch, Handbrake, Shifter, Horn, Wheel Buttons, Encoders, Status LED); `TuningPage.xaml` — card = Button (keyboard-activatable), ek panel, ScottPlot live curve |
| 5 | Setup: Connect → Pins → Calibrate flow | ✅ PASS | `SetupPage.xaml` — 3 steps, chips + progress, Back/Next; Connection/Calibration VMs reuse hote hain (`ViewModelStore`) |
| 6 | Settings: Modules toggles + GPIO pins + Appearance + Data (koi repeat nahi) | ✅ PASS | `SettingsPage.xaml` — Modules (10 toggles, sirf yahin), GPIO pin map + validation + Apply, Appearance (theme switch, sirf yahin), Data (Backup/Restore/Open config folder, sirf yahin) |
| 7 | Copy professional English, koi faltu explanation nahi | ✅ PASS | Aakhri Roman-Urdu user-visible line (`ConnectionPage.xaml` — "Board ka per-device password…") English kar di; baqi Roman-Urdu sirf developer comments mein hai (user ko nazar nahi aata) |
| 8 | RSH branding — "ApexLink" kahin nahi | ✅ PASS | `grep -r ApexLink software/src/RSH.App/` = 0 |
| 9 | Har control asal kaam karta hai (koi fake nahi) | ✅ PASS (static) | Har Button/Slider/Toggle/ComboBox command ya TwoWay binding se wired hai; LED panel mein saaf likha hai ke brightness/pattern "not in this build yet" (fake control ke bajaye sach); Clutch/Handbrake note: wheel axis nahi bhejta — tuning save hoti hai |
| 10 | Performance rules U-P1–U-P10 | ⚠️ PARTIAL (static pass, Windows-PC measure pending) | U-P1: `.Result`/`.Wait()`/`Thread.Sleep` UI code mein 0; U-P2: 33 ms timer (`MainWindow.xaml.cs`); U-P4: 400 ms debounce (tuning save + profile input); U-P5: virtualized Lists (Profiles, Monitor logs); U-P6: ScottPlot redraw throttled (`_lastRefresh`/`_lastPlotRefresh`); U-P10: theme switch sirf dictionaries badalta hai. **Pending (Windows PC):** U-P8 startup ≤ 1200 ms, U-P9 10-min soak memory flat, 30-min real-use freeze test |
| 11 | Light + Dark theme dono kaam karte hain, freeze nahi | ✅ PASS (static + build; runtime switch Windows PC par verify karna hai) | Dark palette + theme-aware Ax* brushes (`RSHTheme.xaml` overrides); theme change ab WPF-UI + RSH palette dono badalta hai (neeche bug B-UI12-2 dekho). Freeze: switch mein koi I/O ya layout rebuild nahi |

## UI-12 mein mile bugs + fixes

| # | Bug | Fix (file:line) |
|---|---|---|
| B-UI12-1 | `SetupPage.xaml:17` Chip style mein hard-coded `#6C4CF1` | `{StaticResource AxAccent}` — ab XAML mein hard-coded `#6C4CF1` = 0 (sirf token files) |
| B-UI12-2 | **Dark theme aadha toota hua:** Settings se theme badalne par sirf WPF-UI theme badalta tha — RSH palette (sidebar/nav) Light par atki rehti thi; `App.ApplyTheme` MainWindow.Background par seedha brush lagata tha (DynamicResource `RSHBackgroundBrush` mar jata tha, spec bg kabhi nahi lagta tha) | `App.xaml.cs:ApplyTheme` ab `RSHThemeManager.ApplyTheme` bhi call karta hai, direct-bg set hata diya; `MainWindow.xaml.cs:Loaded` re-apply barkarar |
| B-UI12-3 | **Pages Dark mein kabhi nahi badalte thay:** Ax* brushes (`AxCard` white, `AxInk` #1F1B2E, `AxMuted` #5F5A7A) static light thay | Palette files mein Color keys + `RSHTheme.xaml` mein theme-aware Ax* brushes (`AxBg/AxInk/AxMutedBrush/AxTrack/AxLine/AxAccentSoft/AxAccentDeep`) aur `AxCard/AxMuted/AxSub/AxGhost` style overrides (window-level, App-level se pehle milte hain) |
| B-UI12-4 | **WCAG-AA fail (Dark):** muted #5F5A7A dark card par 2.74:1; AccentDeep #4B2FD0 dark AccentSoft par 1.80:1; error #B42318 dark card par 2.71:1; warning #B54708 dark card par 3.28:1; banner #FFF4E5 par inherited light text 1.01:1 (na-parhne-layak) | Theme-aware `RSHErrorBrush` (light #B42318 6.6:1 / dark #FF9D94 8.9:1), `RSHWarningTextBrush` (#B54708 5.4:1 / #F5B544 9.8:1), `RSHBannerBrush` (#FFF4E5 / #3A2C10, text se 12–15:1); error/warning literals (`#B42318` ×5 Setup, `#F85149` ×2 Buttons + ×1 Calibration, `#B54708` ×1 Settings) in brushes se badle |
| B-UI12-5 | **WCAG-AA fail (Light):** error #F85149 white par 3.35:1; success green #17803D pill par 4.45:1 | `RSHErrorBrush` (light #B42318 6.6:1); `RSHSuccessTextBrush` #146C35 (pill #E3F6EA par 5.78:1) |
| B-UI12-6 | **Keyboard focus nazar nahi aata tha:** custom button/slider templates (`AxPrimary/AxGhost/AxWhiteBtn/AxSlider/RSH*/ModuleCard/nav`) mein koi focus indicator nahi | `AxFocusVisualStyle` / `RSHFocusVisualStyle` (dashed accent outline) — sab custom styles + `ModuleCard` par |
| B-UI12-7 | Page headings nav naam se nahi milte thay: Dashboard "Home", Monitor "Input Monitor", Output "Output (vJoy)" | Headings nav naam par kar diye (`PageDashboard/PageMonitor/PageOutput` — resx se) |
| B-UI12-8 | Orphaned pages: `DiagnosticsPage` (Monitor mein merge ho chuka) aur `ProcessingPage` (v1 presets — TuningPage us ki jagah leta hai) | Grep-confirm (koi nav/code/test reference nahi — sirf `CalibrationTuning.cs:655` comment) ke baad 4 files delete |

## a11y status

- `AutomationProperties.Name`: **98/98 interactive controls** (MainWindow nav 8, 8 pages, WizardWindow, ChangePasswordDialog) + ButtonsPage ke 32 vJoy cells (code-behind `SetName`). Audit script: `grep`-based XAML scan — 0 missing.
- Keyboard-only flow: nav RadioButtons, module cards (Button), sab buttons/sliders/toggles/combos focusable hain; Tab order = document order (nav upar-se-neeche, phir content); focus visual (B-UI12-6) har custom control par.
- High Contrast: **theme maujood hai** — `RSHTheme.HighContrast.xaml` (`SystemColors`-based; `RSHThemeManager` palette `"HighContrast"`, OS HC on hone par auto). Focus outline + screen-reader names hain. **Runtime verify Windows PC par baqi hai** (HC on karke readability check).
- WCAG-AA contrast: text vs background ≥ 4.5:1 — dono themes mein pass (audit table upar; full pairs UI-12 workstream report mein).

## i18n status

- `Resources/Strings.resx` (English, default) + `Strings.ur.resx` (Urdu) + `Strings.hi.resx` (Hindi) — **337 keys, teeno cultures mein barabar** (generator: `Resources/gen_strings.py`; verified: har file mein 337 `<data>` entries); `Resources/UiStrings.cs` (`{x:Static res:UiStrings.Key}`) — build mein `.resources` embed hote hain (verified: `RSH.App.Resources.Strings[.ur/.hi].resources`).
- Coverage: **52 Text/Content attributes resx se** aate hain (nav 8, headings, main actions — Dashboard/Setup/Profiles/Monitor/Settings/Tuning); ~283 hard-coded literal strings XAML mein baqi hain + VM strings (code-behind) — poori string-by-string rewrite UI-12 scope se bahar thi.
- Runtime language switch **hai**: Settings → Appearance → Language combo (System/en/ur/hi) — `UiStrings.ApplyLanguage` (`App.xaml.cs:273` startup par + Settings change par, restart nahi). `x:Static` bindings `UiStrings.Instance` se refresh hoti hain (`UiStrings.cs:58`).
- Urdu/Hindi translations is workstream mein likhe gaye hain — professional review nahi hua.

## Tests (Linux, 2026-11-13)

- `dotnet build RSH.slnx -c Release`: **0 errors, 0 warnings**.
- `dotnet test tests/RSH.Tests -c Release`: **388 passed, 0 failed, 6 skipped (394 total)** — orphan delete + theme changes se koi regression nahi.
- XAML gates: `#FFB000` = 0, `ApexLink` = 0, hard-coded `#6C4CF1` XAML mein = 0 (sirf token files).
- WPF Linux par run nahi hota — runtime checks (theme switch, screen reader, HC, soak) Windows PC par baqi hain.

## Baqi / pending (khaas taur par Windows PC par)

1. **Runtime theme switch check:** Settings → Appearance → Dark/Light/System — sidebar + pages + ScottPlot colours badalte hain ya nahi (plots ka theme constructor mein ek dafa parha jata hai — runtime switch ke baad plot tab tak purana rehta hai jab tak page dobara na khule; nav dobara kholne par theek ho jata hai).
2. **Screen reader + keyboard-only pass:** Narrator/NVDA se poori app chalana (mouse nahi); har control ka naam parha jaye.
3. **High Contrast mode** on karke readability check (upar dekho).
4. **Soak + startup:** U-P8 startup ≤ 1200 ms, U-P9 10-min soak memory flat, 30-min real use — koi freeze/lag/crash nahi.
5. **Real Steam/game launch:** game launcher asal games ke saath (Steam paths), profile auto-switch + restore-on-exit.
6. **Language setting:** Urdu/Hindi switch ke liye Settings mein option + restart/live reload (resx tayaar hai).
7. **Kept pages note:** `ConnectionPage`/`CalibrationPage` ka koi navigation reference nahi hai (Setup flow unke VMs reuse karta hai, pages nahi) — WS3 ke mutabiq chhede nahi gaye. **FAISLA (Round 2 W7, 2026-11-14, GAP-05): DELETE v2.0.5 mein** (coordinator execute kare, build gate se verify ho) — link karna Setup mein nayi navigation (UI change) hota aur VMs pehle se reuse ho rahe hain, is liye link ka koi faida nahi. Round 2 mein files delete nahi ki gayin (UI LOCKED — sirf faisla).
8. **ScottPlot runtime theme:** theme switch par plots ko dobara rangne ka kaam (constructor-only theme) — chhota follow-up.

## Files changed (UI-12)

- `software/src/RSH.App/Themes/RSHTheme.Light.xaml`, `RSHTheme.Dark.xaml` — Color keys
- `software/src/RSH.App/Themes/RSHTheme.xaml` — theme-aware Ax* brushes/styles, focus style
- `software/src/RSH.App/Themes/Theme.xaml` — focus visuals
- `software/src/RSH.App/App.xaml.cs`, `ViewModels/DiagnosticsSettings.cs` — theme switch fix (B-UI12-2)
- `software/src/RSH.App/MainWindow.xaml`, `Views/Pages/*.xaml` (8 pages + kept pages) — resx wiring, a11y names, contrast fixes, heading parity
- `software/src/RSH.App/Views/Pages/ButtonsPage.xaml.cs` — vJoy cell names
- `software/src/RSH.App/Views/WizardWindow.xaml`, `Views/ChangePasswordDialog.xaml` — a11y names
- `software/src/RSH.App/Resources/Strings.resx`, `Strings.ur.resx`, `Strings.hi.resx`, `UiStrings.cs`, `gen_strings.py` — NEW
- Deleted: `Views/Pages/DiagnosticsPage.xaml(.cs)`, `Views/Pages/ProcessingPage.xaml(.cs)`
- This file: `docs/UI-LOCKED.md` — NEW
