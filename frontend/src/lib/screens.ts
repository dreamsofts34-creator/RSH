import type { ComponentType } from "react";

/**
 * Route discovery (UI plan P2.2).
 *
 * Screens are discovered with `import.meta.glob` over
 * `../screens/<name>/index.tsx` (i.e. `src/screens/<name>/index.tsx`). Each
 * screen module exports:
 *   - `path`: URL-style path segment, e.g. "dashboard"
 *   - `label`: English label shown in the sidebar
 *   - `icon`: a Lucide icon component
 *   - `component`: the screen's React component
 *
 * The shell renders whatever screens exist — Wave 2 workers create the
 * screen files; this module creates none. With zero screens the shell
 * still renders (brand + theme switch + empty content area).
 *
 * Order is the approved screen order (UI plan §0/§5), NOT file order:
 * Dashboard, Tuning, Setup, Buttons, Output, Profiles, Monitor, Settings.
 * A screen whose path is not in the canonical order sorts after the known
 * ones, alphabetically — it still renders, it just cannot jump the queue.
 */

export interface ScreenModule {
  path: string;
  label: string;
  icon: ComponentType<{ className?: string }>;
  component: ComponentType;
}

export interface ScreenDefinition extends ScreenModule {
  /** i18n key for the sidebar label (`nav.<path>`); falls back to `label`. */
  labelKey: string;
}

/** Canonical sidebar order (approved preview order). */
export const NAV_ORDER: readonly string[] = [
  "dashboard",
  "tuning",
  "setup",
  "buttons",
  "output",
  "profiles",
  "monitor",
  "settings",
];

/**
 * React components are functions OR forwardRef/memo exotic objects, so
 * both shapes count as a component here. (lucide-react icons are objects.)
 */
function isComponent(value: unknown): value is ComponentType {
  return (
    value !== null &&
    (typeof value === "function" || typeof value === "object")
  );
}

/**
 * Validate + normalise one globbed module. Returns null for modules that
 * do not export the required shape (logged, never crashing the shell —
 * a broken screen module must not blank the window).
 */
export function toScreenDefinition(
  file: string,
  mod: Partial<ScreenModule> | null | undefined,
): ScreenDefinition | null {
  if (!mod || typeof mod.path !== "string" || mod.path.length === 0) {
    console.warn(`[screens] Ignoring ${file}: missing string export "path".`);
    return null;
  }
  if (typeof mod.label !== "string" || mod.label.length === 0) {
    console.warn(`[screens] Ignoring ${file}: missing string export "label".`);
    return null;
  }
  if (!isComponent(mod.icon)) {
    console.warn(`[screens] Ignoring ${file}: missing component export "icon".`);
    return null;
  }
  if (!isComponent(mod.component)) {
    console.warn(
      `[screens] Ignoring ${file}: missing component export "component".`,
    );
    return null;
  }
  return {
    path: mod.path,
    label: mod.label,
    icon: mod.icon,
    component: mod.component,
    labelKey: `nav.${mod.path}`,
  };
}

/** Sort screens into canonical order; unknown paths go last, alphabetical. */
export function sortScreens(screens: ScreenDefinition[]): ScreenDefinition[] {
  const rank = (path: string): number => {
    const index = NAV_ORDER.indexOf(path);
    return index === -1 ? NAV_ORDER.length : index;
  };
  return [...screens].sort((a, b) => {
    const byRank = rank(a.path) - rank(b.path);
    return byRank !== 0 ? byRank : a.path.localeCompare(b.path);
  });
}

/**
 * Pure discovery step (unit-testable): turn a glob record into sorted
 * screen definitions. `import.meta.glob` supplies the record in production.
 */
export function discoverScreens(
  modules: Record<string, Partial<ScreenModule> | null | undefined>,
): ScreenDefinition[] {
  const found: ScreenDefinition[] = [];
  for (const [file, mod] of Object.entries(modules)) {
    const def = toScreenDefinition(file, mod);
    if (def) found.push(def);
  }
  return sortScreens(found);
}

/**
 * The live glob. `eager: true` so the shell knows the screens at startup
 * without async loading (the shell must paint on first frame).
 *
 * NOTE: Vitest/Vite resolve this at build time. Adding a screen file under
 * src/screens/<name>/index.tsx is enough — no registry edit is needed.
 * (The glob is relative to this file in src/lib/, hence `../screens/`.)
 */
const screenModules = import.meta.glob<Partial<ScreenModule>>(
  "../screens/*/index.tsx",
  { eager: true },
);

export const discoveredScreens: ScreenDefinition[] =
  discoverScreens(screenModules);
