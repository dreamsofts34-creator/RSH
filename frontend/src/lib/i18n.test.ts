import { describe, it, expect, afterEach } from "vitest";
import i18n, { changeLanguage, directionFor } from "./i18n";
import en from "../locales/en.json";
import ur from "../locales/ur.json";
import hi from "../locales/hi.json";

type JsonObject = Record<string, unknown>;

function flattenKeys(obj: JsonObject, prefix = ""): string[] {
  const keys: string[] = [];
  for (const [key, value] of Object.entries(obj)) {
    const full = prefix ? `${prefix}.${key}` : key;
    if (value !== null && typeof value === "object") {
      keys.push(...flattenKeys(value as JsonObject, full));
    } else {
      keys.push(full);
    }
  }
  return keys.sort();
}

describe("i18n infrastructure", () => {
  afterEach(async () => {
    await changeLanguage("en");
  });

  it("has identical key sets in en, ur and hi", () => {
    const enKeys = flattenKeys(en as JsonObject);
    expect(enKeys.length).toBeGreaterThan(0);
    expect(flattenKeys(ur as JsonObject)).toEqual(enKeys);
    expect(flattenKeys(hi as JsonObject)).toEqual(enKeys);
  });

  it("translates shell strings in ur and hi (never blank)", async () => {
    await changeLanguage("ur");
    const urTitle = i18n.t("shell.emptyTitle");
    expect(urTitle.length).toBeGreaterThan(0);
    expect(urTitle).not.toBe("shell.emptyTitle");

    await changeLanguage("hi");
    expect(i18n.t("nav.dashboard")).toBe("डैशबोर्ड");
    expect(i18n.t("appearance.dark")).toBe("गहरा");
  });

  it("falls back to English when a key is missing in ur/hi", async () => {
    // Clone the real bundles but drop one key from ur: the English text
    // must come back instead of a blank string.
    const { createInstance } = await import("i18next");
    const instance = createInstance();
    const urMissing = JSON.parse(JSON.stringify(ur)) as JsonObject;
    const shell = urMissing.shell as JsonObject;
    delete shell.emptyBody;
    await instance.init({
      resources: {
        en: { translation: en },
        ur: { translation: urMissing },
        hi: { translation: hi },
      },
      lng: "ur",
      fallbackLng: "en",
      returnEmptyString: false,
      returnNull: false,
    });
    expect(instance.t("shell.emptyBody")).toBe(
      "No screens are installed yet. Screens arrive with the next build wave.",
    );
  });

  it("never renders blank: a key missing everywhere returns the key itself", () => {
    const missing = i18n.t("does.not.exist.anywhere");
    expect(missing).toBe("does.not.exist.anywhere");
    expect(missing.length).toBeGreaterThan(0);
  });

  it("sets document direction: rtl for Urdu, ltr otherwise", async () => {
    await changeLanguage("ur");
    expect(document.documentElement.dir).toBe("rtl");
    expect(directionFor("ur")).toBe("rtl");
    await changeLanguage("en");
    expect(document.documentElement.dir).toBe("ltr");
    expect(directionFor("en")).toBe("ltr");
    expect(directionFor("hi")).toBe("ltr");
  });

  it("persists the language in localStorage like the theme (H2)", async () => {
    const { loadStoredLanguage, LANGUAGE_STORAGE_KEY } = await import(
      "./i18n"
    );
    window.localStorage.removeItem(LANGUAGE_STORAGE_KEY);
    expect(loadStoredLanguage()).toBe("en");
    await changeLanguage("ur");
    expect(window.localStorage.getItem(LANGUAGE_STORAGE_KEY)).toBe("ur");
    expect(loadStoredLanguage()).toBe("ur");
    window.localStorage.setItem(LANGUAGE_STORAGE_KEY, "xx");
    expect(loadStoredLanguage()).toBe("en");
    await changeLanguage("en");
  });
});
