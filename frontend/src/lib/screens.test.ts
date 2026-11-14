import { describe, it, expect } from "vitest";
import { Gauge, Settings, LayoutDashboard } from "lucide-react";
import {
  discoverScreens,
  discoveredScreens,
  NAV_ORDER,
  toScreenDefinition,
  type ScreenModule,
} from "./screens";

function fakeScreen(path: string, label: string): ScreenModule {
  return { path, label, icon: Gauge, component: () => null };
}

describe("route discovery", () => {
  it("sorts screens into the canonical approved order", () => {
    const found = discoverScreens({
      "./screens/settings/index.tsx": fakeScreen("settings", "Settings"),
      "./screens/dashboard/index.tsx": fakeScreen("dashboard", "Dashboard"),
      "./screens/monitor/index.tsx": fakeScreen("monitor", "Monitor"),
      "./screens/tuning/index.tsx": fakeScreen("tuning", "Tuning"),
    });
    expect(found.map((s) => s.path)).toEqual([
      "dashboard",
      "tuning",
      "monitor",
      "settings",
    ]);
    expect(found[0].labelKey).toBe("nav.dashboard");
  });

  it("puts unknown screen paths last, alphabetically", () => {
    const found = discoverScreens({
      "./screens/zzz/index.tsx": fakeScreen("zzz", "Zzz"),
      "./screens/aaa/index.tsx": fakeScreen("aaa", "Aaa"),
      "./screens/dashboard/index.tsx": fakeScreen("dashboard", "Dashboard"),
    });
    expect(found.map((s) => s.path)).toEqual(["dashboard", "aaa", "zzz"]);
  });

  it("ignores malformed screen modules instead of crashing the shell", () => {
    const warn = (..._args: unknown[]) => {};
    const originalWarn = console.warn;
    console.warn = warn;
    try {
      expect(
        toScreenDefinition("./screens/bad/index.tsx", {
          path: "bad",
          label: "Bad",
        }),
      ).toBeNull();
      expect(
        toScreenDefinition("./screens/bad2/index.tsx", null),
      ).toBeNull();
      expect(
        discoverScreens({
          "./screens/bad/index.tsx": { path: "", label: "Bad" },
          "./screens/ok/index.tsx": fakeScreen("dashboard", "Dashboard"),
        }),
      ).toHaveLength(1);
    } finally {
      console.warn = originalWarn;
    }
  });

  it("canonical order is the approved 8-screen order", () => {
    expect([...NAV_ORDER]).toEqual([
      "dashboard",
      "tuning",
      "setup",
      "buttons",
      "output",
      "profiles",
      "monitor",
      "settings",
    ]);
  });

  it("the live glob returns valid, sorted definitions (zero screens in E1)", () => {
    // E1 creates no screen files: the glob is empty and the shell still
    // renders. (The fixture run proves the glob finds screens added later.)
    expect(Array.isArray(discoveredScreens)).toBe(true);
    for (const screen of discoveredScreens) {
      expect(typeof screen.path).toBe("string");
      expect(["function", "object"]).toContain(typeof screen.component);
    }
    const paths = discoveredScreens.map((s) => s.path);
    const ranked = [...paths].sort((a, b) => {
      const ra = NAV_ORDER.indexOf(a);
      const rb = NAV_ORDER.indexOf(b);
      return (ra === -1 ? 99 : ra) - (rb === -1 ? 99 : rb);
    });
    expect(paths).toEqual(ranked);
    // Keep the icons import used so the fixture-style shape stays honest.
    expect(LayoutDashboard).toBeDefined();
    expect(Settings).toBeDefined();
  });
});
