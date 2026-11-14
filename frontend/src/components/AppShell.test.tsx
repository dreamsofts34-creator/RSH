import { describe, it, expect, afterEach, vi, beforeEach } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { LayoutDashboard, Settings as SettingsIcon } from "lucide-react";
import { AppShell } from "./AppShell";
import { applyTheme } from "../lib/theme";
import type { ScreenDefinition } from "../lib/screens";

function FakeDashboard() {
  return <h1>Dashboard content</h1>;
}

function FakeSettings() {
  return <h1>Settings content</h1>;
}

const fakeScreens: ScreenDefinition[] = [
  {
    path: "settings",
    label: "Settings",
    icon: SettingsIcon,
    component: FakeSettings,
    labelKey: "nav.settings",
  },
  {
    path: "dashboard",
    label: "Dashboard",
    icon: LayoutDashboard,
    component: FakeDashboard,
    labelKey: "nav.dashboard",
  },
];

// Mock at the window.__TAURI__ boundary (same seam as
// src/lib/tauri.test.ts) for the startup-notice command.
const mockInvoke = vi.fn();

function installTauriMock() {
  (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {
    invoke: mockInvoke,
  };
}

function removeTauriMock() {
  delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
}

describe("AppShell", () => {
  beforeEach(() => {
    window.localStorage.clear();
    mockInvoke.mockReset();
    mockInvoke.mockResolvedValue(null);
    installTauriMock();
  });

  afterEach(() => {
    removeTauriMock();
    applyTheme("light");
  });

  it("renders brand and the empty scaffold state with zero screens", () => {
    render(<AppShell screens={[]} />);
    expect(screen.getByText("RSH")).toBeInTheDocument();
    expect(screen.getByText("Welcome to RSH")).toBeInTheDocument();
    expect(
      screen.getByText(
        "No screens are installed yet. Screens arrive with the next build wave.",
      ),
    ).toBeInTheDocument();
    // No theme switch in the sidebar any more (H3): the only theme
    // control lives in Settings → Appearance.
    expect(
      screen.queryByRole("group", { name: "Theme" }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "Dark" }),
    ).not.toBeInTheDocument();
  });

  it("renders discovered screens in canonical order and switches on click", () => {
    render(<AppShell screens={fakeScreens} />);
    const navButtons = screen.getAllByRole("button", { name: /Dashboard|Settings/ });
    // Dashboard sorts before Settings even though it was passed second.
    expect(navButtons[0]).toHaveTextContent("Dashboard");
    expect(navButtons[1]).toHaveTextContent("Settings");
    expect(screen.getByText("Dashboard content")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Settings" }));
    expect(screen.getByText("Settings content")).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Settings" }),
    ).toHaveAttribute("aria-current", "page");
  });

  it("shows the startup notice once and dismisses it", async () => {
    mockInvoke.mockResolvedValue(
      "Store repaired from backup 'bak1': profiles.json restored.",
    );
    render(<AppShell screens={fakeScreens} />);
    await screen.findByText(
      "Store repaired from backup 'bak1': profiles.json restored.",
    );
    fireEvent.click(screen.getByRole("button", { name: "Dismiss" }));
    expect(
      screen.queryByText(
        "Store repaired from backup 'bak1': profiles.json restored.",
      ),
    ).not.toBeInTheDocument();
  });

  it("shows no banner on a clean start", async () => {
    mockInvoke.mockResolvedValue(null);
    render(<AppShell screens={fakeScreens} />);
    expect(screen.getByText("Dashboard content")).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "Dismiss" }),
    ).not.toBeInTheDocument();
  });

  it("remembers the last screen across mounts (M15)", () => {
    const first = render(<AppShell screens={fakeScreens} />);
    fireEvent.click(screen.getByRole("button", { name: "Settings" }));
    expect(screen.getByText("Settings content")).toBeInTheDocument();
    first.unmount();
    // Re-mount: the stored screen is restored instead of the first one.
    render(<AppShell screens={fakeScreens} />);
    expect(screen.getByText("Settings content")).toBeInTheDocument();
  });

  it("Home/End keys jump to the first/last nav item (M15)", () => {
    render(<AppShell screens={fakeScreens} />);
    const settingsButton = screen.getByRole("button", { name: "Settings" });
    fireEvent.click(settingsButton);
    fireEvent.keyDown(settingsButton, { key: "Home" });
    expect(document.activeElement).toBe(
      screen.getByRole("button", { name: "Dashboard" }),
    );
    fireEvent.keyDown(screen.getByRole("button", { name: "Dashboard" }), {
      key: "End",
    });
    expect(document.activeElement).toBe(
      screen.getByRole("button", { name: "Settings" }),
    );
  });
});
