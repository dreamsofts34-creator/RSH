import "@testing-library/jest-dom/vitest";
import { cleanup } from "@testing-library/react";
import { afterEach } from "vitest";
import "../lib/i18n";
import { applyTheme } from "../lib/theme";

// jsdom has no `ResizeObserver` (Radix Slider needs it): stub it so slider
// tests exercise the component code instead of the environment gap.
// Production (WebView2) always provides it.
//
// NOTE: deliberately NO `matchMedia` stub here. uPlot reads `matchMedia`
// at module scope; the chart components lazy-import uPlot and fall back to
// an SVG curve when the import fails (E2/E5 design). Stubbing `matchMedia`
// would let the import succeed in jsdom and then fail later on the missing
// canvas 2D context — hiding the fallback path the tests cover.

if (typeof window !== "undefined" && window.ResizeObserver === undefined) {
  window.ResizeObserver = class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
}

// Vitest runs with globals: false, so Testing Library's auto-cleanup is
// not registered — clean up the DOM after every test explicitly.
afterEach(() => {
  cleanup();
});

// Tests run with a clean DOM per file; start each file in the Light theme
// so theme tests assert transitions explicitly.
applyTheme("light");
