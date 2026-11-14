# Frontend — Phase E (shell + navigation + themes + i18n + 8 screens)

The web frontend for RSH v1.0.0 (Tauri 2 app): **Vite + React 19 +
TypeScript (strict) + Tailwind CSS v4 + shadcn/ui + Lucide + react-i18next
+ uPlot**, per `RSH-RUST-UI-PLAN.md` P1 + P2.

## What E1 built

- **Scaffold** — Vite build (`npm run dev` on port 1420, `npm run build`
  emits `dist/`). No `tailwind.config.*`, no PostCSS; Tailwind v4 via
  `@tailwindcss/vite` and `@import "tailwindcss"` in `src/index.css`.
- **Tokens / theming (P1)** — design tokens from the C# `RSHTheme.*.xaml`
  palettes as CSS custom properties in `src/index.css`: Light (default,
  bg `#F5F3FF`), Dark (bg `#0B0E13`), High Contrast, accent violet
  `#6C4CF1` everywhere, no amber. Theme switching lives ONLY in the
  shell's Appearance control (`src/components/ThemeSwitch.tsx`); applied
  by flipping `data-theme` on `<html>` (`src/lib/theme.ts`).
- **Shell + navigation (P2)** — `src/components/AppShell.tsx`: brand
  sidebar, content area, window title `RSH — Real Sim Hub v1.0.0`.
  Screens are discovered with `import.meta.glob` over
  `src/screens/<name>/index.tsx` (`src/lib/screens.ts`); each screen module
  exports `{ path, label, icon, component }`. Wave 2 added the 8 screens
  (Dashboard, Tuning, Setup, Buttons, Output, Profiles, Monitor,
  Settings); with zero screens the shell renders brand + theme switch +
  literal scaffold text (never a blank window).
- **i18n (P10 foundation)** — `react-i18next`, bundles in
  `src/locales/{en,ur,hi}.json` (shell/nav/theme strings in E1; screen
  strings arrive with Wave 2). English fallback on ANY missing
  key, so text is never blank. Language switching lives ONLY in Settings
  (Wave 2); this phase wires the infrastructure + `en` default.
- **Locales (Phase E gate)** — screen strings live per-screen in
  `src/screens/<name>/strings.{en,ur,hi}.json` (namespaced keys, one
  file per locale) and are merged into `src/locales/{en,ur,hi}.json` by
  `scripts/merge-locales.mjs`. **After editing any screen's strings,
  re-run `node scripts/merge-locales.mjs`** (it deep-merges, fails loudly
  on key collisions, locale key-set mismatches, or single-brace `{var}`
  interpolation — all interpolations must be `{{var}}`). Verify with
  `node scripts/merge-locales.mjs --check` (CI runs the check). Never
  hand-edit `src/locales/*.json` for screen keys — the next merge
  overwrites them.
- **Tauri contract** — `src/lib/tauri.ts`: typed `invoke` wrappers for
  the full command surface (snapshot, transport, pairing, config
  stage/commit/confirm/rollback, pin map, profiles, games, settings,
  firmware upload). `ping`, `get_version`, `validate_pin_map`, and
  `validate_firmware_image` exist in `src-tauri` today (Phase E gate);
  every other wrapper fails loudly with a typed
  `TauriCommandError` (`NOT_IMPLEMENTED`) — never fake data.
- **shadcn/ui** — components copied into `src/components/ui/`:
  button, card, dialog, input, label, select, separator, slider, switch,
  tabs, badge, tooltip, progress, scroll-area.

## Commands

| Command | What it does |
| --- | --- |
| `npm install` | Install pinned dependencies |
| `npm run dev` | Vite dev server (port 1420, strict) |
| `npm run build` | `tsc -b && vite build` → `frontend/dist/` |
| `npm test` | Vitest (unit/component, jsdom) |
| `npm run lint` | ESLint (flat config) |
| `node scripts/merge-locales.mjs` | Merge per-screen `strings.*.json` into `src/locales/*.json` (re-run after ANY screen-string edit; `--check` verifies parity) |

## Notes

- Add a screen by creating `src/screens/<name>/index.tsx` exporting
  `path`, `label`, `icon`, `component` — no registry edit needed. The
  canonical nav order is `NAV_ORDER` in `src/lib/screens.ts`.
- Screen strings go in `src/screens/<name>/strings.{en,ur,hi}.json`
  (all three locales, identical key sets) — then re-run
  `node scripts/merge-locales.mjs` to merge them into
  `src/locales/*.json`. Never hand-edit the merged files for screen keys.
- Tauri command Rust implementations are Windows-CI-only; keep the
  `NOT_YET_IMPLEMENTED_COMMANDS` list in `src/lib/tauri.ts` in sync with
  `src-tauri/src/main.rs` as commands land.
- Visual paint check (real window on Windows) is still open — the tests
  assert theme class + token values, not pixels.
