import { describe, it, expect, beforeEach, afterEach } from "vitest";
import cssText from "../index.css?raw";
import {
  applyTheme,
  contrastRatio,
  loadStoredTheme,
  resolveTheme,
  storeTheme,
  THEME_IDS,
  THEME_STORAGE_KEY,
  THEME_TOKENS,
  type ResolvedTheme,
} from "./theme";

describe("theme switching", () => {
  beforeEach(() => {
    window.localStorage.clear();
    applyTheme("light");
  });

  afterEach(() => {
    applyTheme("light");
  });

  it("applies Light: data-theme=light and no dark class", () => {
    expect(applyTheme("light")).toBe("light");
    expect(document.documentElement.getAttribute("data-theme")).toBe("light");
    expect(document.documentElement.classList.contains("dark")).toBe(false);
    expect(
      document.documentElement.classList.contains("high-contrast"),
    ).toBe(false);
  });

  it("applies Dark: data-theme=dark plus the dark class", () => {
    expect(applyTheme("dark")).toBe("dark");
    expect(document.documentElement.getAttribute("data-theme")).toBe("dark");
    expect(document.documentElement.classList.contains("dark")).toBe(true);
  });

  it("applies High Contrast: data-theme=high-contrast plus its class", () => {
    expect(applyTheme("high-contrast")).toBe("high-contrast");
    expect(document.documentElement.getAttribute("data-theme")).toBe(
      "high-contrast",
    );
    expect(
      document.documentElement.classList.contains("high-contrast"),
    ).toBe(true);
    expect(document.documentElement.classList.contains("dark")).toBe(false);
  });

  it("resolves system theme from the OS preference", () => {
    const original = window.matchMedia;
    window.matchMedia = ((query: string) => ({
      matches: query === "(prefers-color-scheme: dark)",
      media: query,
      addEventListener: () => {},
      removeEventListener: () => {},
    })) as unknown as typeof window.matchMedia;
    try {
      expect(resolveTheme("system")).toBe("dark");
      expect(applyTheme("system")).toBe("dark");
    } finally {
      window.matchMedia = original;
    }
  });

  it("persists the choice and falls back to Light on bad stored data", () => {
    storeTheme("dark");
    expect(loadStoredTheme()).toBe("dark");
    window.localStorage.setItem(THEME_STORAGE_KEY, "not-a-theme");
    expect(loadStoredTheme()).toBe("light");
    window.localStorage.removeItem(THEME_STORAGE_KEY);
    expect(loadStoredTheme()).toBe("light");
  });

  it("knows exactly the four theme ids", () => {
    expect([...THEME_IDS]).toEqual([
      "light",
      "dark",
      "high-contrast",
      "system",
    ]);
  });
});

describe("theme tokens resolve + WCAG-AA contrast", () => {
  // Worst pairs from the C# Round 2 / UI-12 audit — every pair must stay
  // >= 4.5:1 (WCAG-AA for normal text) in every theme.
  const pairs: {
    theme: ResolvedTheme;
    fg: string;
    bg: string;
    label: string;
  }[] = [
    { theme: "light", fg: "--ink", bg: "--bg", label: "ink on bg" },
    { theme: "light", fg: "--muted", bg: "--bg", label: "muted on bg" },
    { theme: "light", fg: "--ink", bg: "--card", label: "ink on card" },
    { theme: "light", fg: "--muted", bg: "--card", label: "muted on card" },
    { theme: "light", fg: "--on-accent", bg: "--accent", label: "on-accent on accent" },
    { theme: "light", fg: "--success-text", bg: "--success-pill", label: "success text on pill" },
    { theme: "light", fg: "--error-text", bg: "--card", label: "error text on card" },
    { theme: "light", fg: "--warning-text", bg: "--card", label: "warning text on card" },
    { theme: "light", fg: "--nav-active-fg", bg: "--nav-active-bg", label: "nav active fg on bg" },
    { theme: "light", fg: "--accent-deep", bg: "--bg", label: "accent-deep on bg" },
    { theme: "light", fg: "--accent", bg: "--card", label: "accent on card" },
    { theme: "light", fg: "--accent-deep", bg: "--card", label: "accent-deep on card" },
    { theme: "light", fg: "--muted", bg: "--muted-pill", label: "muted on muted pill" },
    { theme: "light", fg: "--error-text", bg: "--danger-pill", label: "error text on danger pill" },
    { theme: "light", fg: "--success-text", bg: "--card", label: "success text on card" },
    { theme: "dark", fg: "--ink", bg: "--bg", label: "ink on bg" },
    { theme: "dark", fg: "--muted", bg: "--bg", label: "muted on bg" },
    { theme: "dark", fg: "--ink", bg: "--card", label: "ink on card" },
    { theme: "dark", fg: "--muted", bg: "--card", label: "muted on card" },
    { theme: "dark", fg: "--on-accent", bg: "--accent", label: "on-accent on accent" },
    { theme: "dark", fg: "--success-text", bg: "--success-pill", label: "success text on pill" },
    { theme: "dark", fg: "--error-text", bg: "--card", label: "error text on card" },
    { theme: "dark", fg: "--warning-text", bg: "--card", label: "warning text on card" },
    { theme: "dark", fg: "--nav-active-fg", bg: "--nav-active-bg", label: "nav active fg on bg" },
    { theme: "dark", fg: "--accent-deep", bg: "--bg", label: "accent-deep on bg" },
    { theme: "dark", fg: "--accent-deep", bg: "--card", label: "accent-deep on card" },
    { theme: "dark", fg: "--muted", bg: "--muted-pill", label: "muted on muted pill" },
    { theme: "dark", fg: "--error-text", bg: "--danger-pill", label: "error text on danger pill" },
    { theme: "dark", fg: "--success-text", bg: "--card", label: "success text on card" },
    { theme: "high-contrast", fg: "--ink", bg: "--bg", label: "ink on bg" },
    { theme: "high-contrast", fg: "--muted", bg: "--bg", label: "muted on bg" },
    { theme: "high-contrast", fg: "--on-accent", bg: "--accent", label: "on-accent on accent" },
    { theme: "high-contrast", fg: "--nav-active-fg", bg: "--nav-active-bg", label: "nav active fg on bg" },
    { theme: "high-contrast", fg: "--muted", bg: "--card", label: "muted on card" },
    { theme: "high-contrast", fg: "--success-text", bg: "--success-pill", label: "success text on pill" },
    { theme: "high-contrast", fg: "--error-text", bg: "--danger-pill", label: "error text on danger pill" },
    { theme: "high-contrast", fg: "--warning-text", bg: "--card", label: "warning text on card" },
    { theme: "high-contrast", fg: "--accent-deep", bg: "--bg", label: "accent-deep on bg" },
  ];
  // NOTE (H3 audit): dark `--accent` (#6C4CF1) as text on the dark card is
  // 3.34:1 — below AA — but no component renders accent-coloured text on
  // a dark card (grep: no text-accent/text-primary/text-info usage), so
  // it is deliberately NOT in the list above. If a component ever does,
  // add the pair here and it will fail until the colour is fixed.

  it.each(pairs)(
    "$theme: $label is WCAG-AA (>= 4.5:1)",
    ({ theme, fg, bg }) => {
      const tokens = THEME_TOKENS[theme];
      const ratio = contrastRatio(tokens[fg], tokens[bg]);
      expect(ratio).toBeGreaterThanOrEqual(4.5);
    },
  );

  it("uses the violet accent in Light and Dark, never amber", () => {
    for (const theme of ["light", "dark"] as const) {
      expect(THEME_TOKENS[theme]["--accent"]).toBe("#6C4CF1");
    }
    const all = Object.values(THEME_TOKENS).flatMap((t) => Object.values(t));
    // No amber accent anywhere (#F59E0B is allowed ONLY as the warning dot).
    expect(all.filter((v) => v === "#F59E0B").length).toBeLessThanOrEqual(3);
  });

  it("token values in theme.ts match the values in index.css (no drift)", () => {
    const css = cssText.toLowerCase();
    for (const [theme, tokens] of Object.entries(THEME_TOKENS)) {
      for (const [name, value] of Object.entries(tokens)) {
        expect(
          css.includes(value.toLowerCase()),
          `${theme} ${name} ${value} must appear in index.css`,
        ).toBe(true);
      }
    }
  });
});
