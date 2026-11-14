/**
 * Theme handling (UI plan P1.2) — the ONLY place themes are applied.
 *
 * Themes: `light` (default), `dark`, `high-contrast`, `system` (follows the
 * OS colour scheme; High Contrast is picked up via `forced-colors` CSS).
 * Applied by setting `data-theme` + a matching class on <html> — a single
 * attribute flip, no DOM rebuild (R-P10). The choice persists in
 * localStorage under THEME_STORAGE_KEY; nothing else in the app touches
 * the theme.
 *
 * THEME_TOKENS mirrors src/index.css exactly (same hex values) so tests
 * can verify contrast ratios and CSS drift without a browser.
 */

export type ThemeId = "light" | "dark" | "high-contrast" | "system";
export type ResolvedTheme = "light" | "dark" | "high-contrast";

export const THEME_IDS: readonly ThemeId[] = [
  "light",
  "dark",
  "high-contrast",
  "system",
];

export const THEME_STORAGE_KEY = "rsh-theme";

/** Token values per theme — must match src/index.css (drift test enforces it). */
export const THEME_TOKENS: Record<
  Exclude<ResolvedTheme, "system">,
  Record<string, string>
> = {
  light: {
    "--accent": "#6C4CF1",
    "--accent-deep": "#4B2FD0",
    "--accent-hover": "#5A3BE0",
    "--accent-soft": "#EEE9FF",
    "--bg": "#F5F3FF",
    "--card": "#FFFFFF",
    "--raised": "#FFFFFF",
    "--ink": "#1F1B2E",
    "--muted": "#5F5A7A",
    "--track": "#ECE8FA",
    "--line": "#E7E3F7",
    "--success": "#34D399",
    "--success-text": "#146C35",
    "--success-pill": "#E3F6EA",
    "--warning": "#F59E0B",
    "--warning-text": "#B54708",
    "--danger": "#FF4D4F",
    "--error-text": "#B42318",
    "--danger-pill": "#FDE8E8",
    "--muted-pill": "#EDEBF5",
    "--info": "#22D3EE",
    "--on-accent": "#FFFFFF",
    "--banner": "#FFF4E5",
    "--nav-active-bg": "#EEE9FF",
    "--nav-hover-bg": "#F5F3FF",
    "--nav-active-fg": "#4B2FD0",
    "--nav-dot": "#C9C1EE",
  },
  dark: {
    "--accent": "#6C4CF1",
    "--accent-deep": "#C4B5FD",
    "--accent-hover": "#5A3BE0",
    "--accent-soft": "#2A2350",
    "--bg": "#0B0E13",
    "--card": "#14181F",
    "--raised": "#1A1F27",
    "--ink": "#F2F4F7",
    "--muted": "#9AA3AF",
    "--track": "#232936",
    "--line": "#262C36",
    "--success": "#34D399",
    "--success-text": "#7ED491",
    "--success-pill": "#1D3A28",
    "--warning": "#F59E0B",
    "--warning-text": "#F5B544",
    "--danger": "#FF4D4F",
    "--error-text": "#FF9D94",
    "--danger-pill": "#3D1F1F",
    "--muted-pill": "#232936",
    "--info": "#22D3EE",
    "--on-accent": "#FFFFFF",
    "--banner": "#3A2C10",
    "--nav-active-bg": "#2A2350",
    "--nav-hover-bg": "#1A1F27",
    "--nav-active-fg": "#C4B5FD",
    "--nav-dot": "#4A445F",
  },
  "high-contrast": {
    "--accent": "#FFFF00",
    "--accent-deep": "#FFFFFF",
    "--accent-hover": "#FFFF00",
    "--accent-soft": "#1A1A1A",
    "--bg": "#000000",
    "--card": "#000000",
    "--raised": "#1A1A1A",
    "--ink": "#FFFFFF",
    "--muted": "#FFFFFF",
    "--track": "#3A3A3A",
    "--line": "#FFFFFF",
    "--success": "#00E676",
    "--success-text": "#FFFFFF",
    "--success-pill": "#1A1A1A",
    "--warning": "#FFC107",
    "--warning-text": "#FFFFFF",
    "--danger": "#FF5252",
    "--error-text": "#FFFFFF",
    "--danger-pill": "#1A1A1A",
    "--muted-pill": "#1A1A1A",
    "--info": "#40C4FF",
    "--on-accent": "#000000",
    "--banner": "#000000",
    "--nav-active-bg": "#FFFF00",
    "--nav-hover-bg": "#1A1A1A",
    "--nav-active-fg": "#000000",
    "--nav-dot": "#FFFFFF",
  },
};

export function isThemeId(value: unknown): value is ThemeId {
  return (
    typeof value === "string" &&
    (THEME_IDS as readonly string[]).includes(value)
  );
}

/** Resolve `system` against the OS preference; anything else passes through. */
export function resolveTheme(theme: ThemeId): ResolvedTheme {
  if (theme !== "system") return theme;
  if (typeof window === "undefined" || !window.matchMedia) return "light";
  return window.matchMedia("(prefers-color-scheme: dark)").matches
    ? "dark"
    : "light";
}

/**
 * Apply a theme: set `data-theme` + the matching class on <html>.
 * Returns the resolved theme. Never throws — a theme failure must never
 * leave the window unstyled.
 */
export function applyTheme(theme: ThemeId): ResolvedTheme {
  const resolved = resolveTheme(theme);
  try {
    const root = document.documentElement;
    root.setAttribute("data-theme", resolved);
    root.classList.toggle("dark", resolved === "dark");
    root.classList.toggle("high-contrast", resolved === "high-contrast");
  } catch {
    // Non-DOM environment (tests without jsdom, SSR): nothing to apply.
  }
  return resolved;
}

/** Read the persisted theme choice; defaults to `light` on bad/missing data. */
export function loadStoredTheme(): ThemeId {
  try {
    const stored = window.localStorage.getItem(THEME_STORAGE_KEY);
    if (isThemeId(stored)) return stored;
  } catch {
    // Storage unavailable — fall through to the default.
  }
  return "light";
}

export function storeTheme(theme: ThemeId): void {
  try {
    window.localStorage.setItem(THEME_STORAGE_KEY, theme);
  } catch {
    // Storage unavailable — the theme still applies for this session.
  }
}

/** Apply + persist a theme choice. Used by the shell's Appearance control. */
export function setTheme(theme: ThemeId): ResolvedTheme {
  const resolved = applyTheme(theme);
  storeTheme(theme);
  return resolved;
}

/* ------------------------------------------------------------------ */
/* WCAG contrast helpers (used by tests; pure functions, no DOM).      */
/* ------------------------------------------------------------------ */

function hexToRgb(hex: string): [number, number, number] {
  const clean = hex.replace("#", "");
  const full =
    clean.length === 3
      ? clean
          .split("")
          .map((c) => c + c)
          .join("")
      : clean;
  const value = Number.parseInt(full, 16);
  return [(value >> 16) & 0xff, (value >> 8) & 0xff, value & 0xff];
}

function relativeLuminance(hex: string): number {
  const [r, g, b] = hexToRgb(hex).map((channel) => {
    const s = channel / 255;
    return s <= 0.03928 ? s / 12.92 : Math.pow((s + 0.055) / 1.055, 2.4);
  });
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

/** WCAG contrast ratio between two hex colours (1–21). */
export function contrastRatio(foreground: string, background: string): number {
  const l1 = relativeLuminance(foreground);
  const l2 = relativeLuminance(background);
  const [lighter, darker] = l1 >= l2 ? [l1, l2] : [l2, l1];
  return (lighter + 0.05) / (darker + 0.05);
}
