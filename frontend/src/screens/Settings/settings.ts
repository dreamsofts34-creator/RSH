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
  if (isSupportedLanguage(raw.language)) next.language = raw.language;
  return next;
}

/** Serialisable payload for `set_settings`. */
export function settingsPayload(state: SettingsState): Record<string, unknown> {
  return {
    modules: { ...state.modules },
    theme: state.theme,
    language: state.language,
  };
}
