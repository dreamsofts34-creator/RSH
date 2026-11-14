import type { TFunction } from "i18next";
import { isSupportedLanguage, type SupportedLanguage } from "@/lib/i18n";
import { isThemeId, type ThemeId } from "@/lib/theme";

/**
 * Settings screen (UI plan P9) — shared model.
 *
 * The 10 modules are the single source of truth for the on/off state:
 * the same `modules` map the Tuning cards read lives here in the
 * persisted settings (via the `get_settings` / `set_settings` wrappers),
 * never in a second copy. The GPIO pin map uses the same 10 function ids.
 *
 * `t()` lookups below carry an English `defaultValue` because the
 * `settings.*` strings live in this screen's `strings.{en,ur,hi}.json`
 * until the coordinator merges them into `src/locales/` at the Phase E
 * gate — a missing bundle must fall back to English, never to blank.
 */

/** The 10 modules (UI plan P4.1 / P9.1), in canonical order. */
export const MODULE_IDS = [
  "steering",
  "accelerator",
  "brake",
  "clutch",
  "handbrake",
  "shifter",
  "horn",
  "buttons",
  "encoders",
  "led",
] as const;

export type ModuleId = (typeof MODULE_IDS)[number];

export type ModuleStates = Record<ModuleId, boolean>;

export interface SettingsState {
  modules: ModuleStates;
  theme: ThemeId;
  language: SupportedLanguage;
}

export const DEFAULT_SETTINGS: SettingsState = {
  modules: {
    steering: true,
    accelerator: true,
    brake: true,
    clutch: true,
    handbrake: true,
    shifter: true,
    horn: true,
    buttons: true,
    encoders: true,
    led: true,
  },
  theme: "light",
  language: "en",
};

/** Translate with an English fallback (see the note above). */
export function st(
  t: TFunction,
  key: string,
  fallback: string,
  vars?: Record<string, string | number>,
): string {
  return t(key, { defaultValue: fallback, ...(vars ?? {}) });
}

/** Module label key, e.g. `settings.modules.steering`. */
export function moduleLabelKey(id: ModuleId): string {
  return `settings.modules.${id}`;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/**
 * Parse the untyped `get_settings` payload defensively: unknown or
 * missing fields fall back to the defaults instead of crashing the
 * screen or inventing a state the backend never reported.
 *
 * Theme accepts BOTH vocabularies: the frontend ids
 * (`light`/`dark`/`high-contrast`/`system`) and the backend canonical
 * vocabulary (`Light`/`Dark`/`HighContrast`/`System`, C-SET-1) — the
 * backend record may arrive in either form depending on which side
 * wrote it last.
 */
export function parseSettings(raw: unknown): SettingsState {
  const next: SettingsState = {
    modules: { ...DEFAULT_SETTINGS.modules },
    theme: DEFAULT_SETTINGS.theme,
    language: DEFAULT_SETTINGS.language,
  };
  if (!isRecord(raw)) return next;
  if (isRecord(raw.modules)) {
    for (const id of MODULE_IDS) {
      if (typeof raw.modules[id] === "boolean") {
        next.modules[id] = raw.modules[id];
      }
    }
  }
  if (isThemeId(raw.theme)) next.theme = raw.theme;
  else {
    const fromBackend = themeFromBackend(raw.theme);
    if (fromBackend !== null) next.theme = fromBackend;
  }
  if (isSupportedLanguage(raw.language)) next.language = raw.language;
  return next;
}

/**
 * Backend theme vocabulary (C-SET-1, canonical in `AppSettings`):
 * `System` | `Dark` | `Light` | `HighContrast`.
 */
export const BACKEND_THEME_IDS = [
  "System",
  "Dark",
  "Light",
  "HighContrast",
] as const;

export type BackendThemeId = (typeof BACKEND_THEME_IDS)[number];

const TO_BACKEND: Record<ThemeId, BackendThemeId> = {
  light: "Light",
  dark: "Dark",
  "high-contrast": "HighContrast",
  system: "System",
};

const FROM_BACKEND: Record<BackendThemeId, ThemeId> = {
  Light: "light",
  Dark: "dark",
  HighContrast: "high-contrast",
  System: "system",
};

/** Frontend theme id -> backend canonical vocabulary. */
export function themeToBackend(theme: ThemeId): BackendThemeId {
  return TO_BACKEND[theme];
}

/** Backend canonical vocabulary -> frontend theme id, or null when unknown. */
export function themeFromBackend(value: unknown): ThemeId | null {
  if (typeof value !== "string") return null;
  return (FROM_BACKEND as Record<string, ThemeId>)[value] ?? null;
}

/**
 * Serialisable payload for `set_settings` (C1/C-SET-1).
 *
 * The backend merges this record (PATCH semantics): the caller must
 * pass the FULL record — see `fullSettingsPayload` — so fields this
 * screen does not own (esp_ip, autostart, …) are never dropped by an
 * older non-merge backend. The theme is sent in the backend canonical
 * vocabulary and the `modules` map is always sent in full.
 */
export function settingsPayload(state: SettingsState): Record<string, unknown> {
  return {
    modules: { ...state.modules },
    theme: themeToBackend(state.theme),
    language: state.language,
  };
}

/**
 * Full-record payload: the raw record last returned by `get_settings`
 * with this screen's fields overlaid. Unknown backend fields ride
 * along untouched, so a save can never wipe a setting it never saw.
 */
export function fullSettingsPayload(
  raw: Record<string, unknown> | null,
  state: SettingsState,
): Record<string, unknown> {
  return { ...(raw ?? {}), ...settingsPayload(state) };
}
