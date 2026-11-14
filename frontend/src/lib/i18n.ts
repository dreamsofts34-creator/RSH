import i18n from "i18next";
import { initReactI18next } from "react-i18next";
import en from "../locales/en.json";
import ur from "../locales/ur.json";
import hi from "../locales/hi.json";

/**
 * i18n infrastructure (UI plan P10.1 foundation).
 *
 * - Bundles: src/locales/{en,ur,hi}.json. English is the fallback for ANY
 *   missing key, so text is never blank (the C# blank-window lesson: a
 *   missing resource must degrade to English, never to an empty string).
 * - `returnEmptyString: false` + `returnNull: false`: even an empty
 *   translation falls back instead of rendering nothing.
 * - A key missing from every bundle renders as the key itself — visible
 *   in development, never blank in production.
 * - Language switching is done here only by Settings (Wave 2); this module
 *   only wires the infrastructure and defaults to English.
 */

export const SUPPORTED_LANGUAGES = ["en", "ur", "hi"] as const;
export type SupportedLanguage = (typeof SUPPORTED_LANGUAGES)[number];

export const DEFAULT_LANGUAGE: SupportedLanguage = "en";

/** Urdu is RTL; English and Hindi render LTR. */
export function directionFor(language: string): "ltr" | "rtl" {
  return language === "ur" ? "rtl" : "ltr";
}

export function isSupportedLanguage(value: unknown): value is SupportedLanguage {
  return (
    typeof value === "string" &&
    (SUPPORTED_LANGUAGES as readonly string[]).includes(value)
  );
}

void i18n.use(initReactI18next).init({
  resources: {
    en: { translation: en },
    ur: { translation: ur },
    hi: { translation: hi },
  },
  lng: DEFAULT_LANGUAGE,
  fallbackLng: DEFAULT_LANGUAGE,
  // Never render a blank string: fall back to English, then to the key.
  returnEmptyString: false,
  returnNull: false,
  interpolation: { escapeValue: false },
  react: { useSuspense: false },
});

/** Switch language at runtime (no restart) and set document direction. */
export async function changeLanguage(
  language: SupportedLanguage,
): Promise<void> {
  await i18n.changeLanguage(language);
  if (typeof document !== "undefined") {
    document.documentElement.lang = language;
    document.documentElement.dir = directionFor(language);
  }
}

export default i18n;
