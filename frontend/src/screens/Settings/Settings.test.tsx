import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent, waitFor, within } from "@testing-library/react";
import i18n, { changeLanguage } from "@/lib/i18n";
import { applyTheme } from "@/lib/theme";
import SettingsScreen from "./index";
import { parseSettings } from "./settings";
import enStrings from "./strings.en.json";
import urStrings from "./strings.ur.json";
import hiStrings from "./strings.hi.json";

// Mocks live at the window.__TAURI__ boundary only (same seam as
// src/lib/tauri.test.ts). No backend behaviour is faked in production
// code — these doubles stand in for the Rust commands.
const mockInvoke = vi.fn();

function installTauriMock() {
  (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {
    invoke: mockInvoke,
  };
}

function removeTauriMock() {
  delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
}

const SETTINGS_PAYLOAD = {
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

const PIN_MAP = [
  { function: "steering", pin: 32 },
  { function: "accelerator", pin: 33 },
  { function: "brake", pin: 34 },
  { function: "clutch", pin: 35 },
  { function: "handbrake", pin: 36 },
  { function: "shifter", pin: 39 },
  { function: "horn", pin: 4 },
  { function: "buttons", pin: 5 },
  { function: "encoders", pin: 13 },
  { function: "led", pin: 21 },
];

let failGetSettingsNotImplemented = false;

function setSettingsCall(): Record<string, unknown> | undefined {
  const call = mockInvoke.mock.calls.find((c) => c[0] === "set_settings");
  return (call?.[1] as { settings?: Record<string, unknown> } | undefined)
    ?.settings;
}

describe("Settings screen", () => {
  beforeEach(() => {
    i18n.addResourceBundle("en", "translation", enStrings, true, true);
    i18n.addResourceBundle("ur", "translation", urStrings, true, true);
    i18n.addResourceBundle("hi", "translation", hiStrings, true, true);
    failGetSettingsNotImplemented = false;
    mockInvoke.mockReset();
    mockInvoke.mockImplementation(
      (command: string, args?: Record<string, unknown>) => {
        switch (command) {
          case "get_settings":
            if (failGetSettingsNotImplemented) {
              return Promise.reject(
                new Error('unknown command "get_settings"'),
              );
            }
            return Promise.resolve(SETTINGS_PAYLOAD);
          case "set_settings":
            return Promise.resolve(args?.settings ?? {});
          case "get_transport_state":
            return Promise.resolve({
              transport: "wifi",
              state: "connected",
              address: "192.168.4.1",
              error: null,
            });
          case "switch_transport":
            return Promise.resolve({
              transport: args?.transport ?? "wifi",
              state: "connected",
              address: args?.address ?? "",
              error: null,
            });
          case "get_autostart":
            return Promise.resolve(false);
          case "set_autostart":
            return Promise.resolve(args?.enabled ?? false);
          case "get_pin_map":
            return Promise.resolve(PIN_MAP);
          case "validate_pin_map":
            return Promise.resolve({ valid: true, errors: [], warnings: [] });
          case "set_pin_map":
            return Promise.resolve({ valid: true, errors: [], warnings: [] });
          case "confirm_config":
            return Promise.resolve("clean");
          case "rollback_config":
            return Promise.resolve("rolled_back");
          case "backup_now":
            return Promise.resolve([
              {
                generation: "current",
                present: true,
                savedAt: "2026-11-14T12:00:00Z",
              },
              {
                generation: "bak1",
                present: true,
                savedAt: "2026-11-13T12:00:00Z",
              },
              { generation: "bak2", present: false, savedAt: null },
            ]);
          case "list_backups":
            return Promise.resolve([
              {
                generation: "current",
                present: true,
                savedAt: "2026-11-14T12:00:00Z",
              },
              {
                generation: "bak1",
                present: true,
                savedAt: "2026-11-13T12:00:00Z",
              },
              { generation: "bak2", present: false, savedAt: null },
            ]);
          case "get_firewall_state":
            return Promise.resolve({
              rulePresent: false,
              detail: "No inbound rule for RSH.exe.",
            });
          case "restore_backup":
            return Promise.resolve(undefined);
          case "open_config_folder":
            return Promise.resolve(undefined);
          default:
            return Promise.reject(
              new Error(`unknown command "${command}"`),
            );
        }
      },
    );
    installTauriMock();
  });

  afterEach(async () => {
    removeTauriMock();
    applyTheme("light");
    await changeLanguage("en");
  });

  it("renders every section", async () => {
    render(<SettingsScreen />);
    expect(
      screen.getByRole("heading", { name: "Settings" }),
    ).toBeInTheDocument();
    await screen.findByText(
      "Turn modules on or off. Changes apply immediately.",
    );
    expect(
      screen.getByText(
        "Assign a GPIO pin to each function. The map is checked by the board validator before it is applied.",
      ),
    ).toBeInTheDocument();
    expect(
      screen.getByText("Theme and language. Changes apply immediately."),
    ).toBeInTheDocument();
    expect(
      screen.getByText("How the app connects to the wheel."),
    ).toBeInTheDocument();
    expect(
      screen.getByText("Back up and restore configuration data."),
    ).toBeInTheDocument();
    expect(
      screen.getByText("Startup and operating system integration."),
    ).toBeInTheDocument();
    // H3: the real 3-generation backup list is shown (list_backups).
    expect(screen.getByText("Saved backups")).toBeInTheDocument();
    await screen.findByText("Current");
    expect(screen.getByText("Backup 1")).toBeInTheDocument();
  });

  it("module toggle calls set_settings with the updated modules map", async () => {
    render(<SettingsScreen />);
    await screen.findByText("Modules");
    fireEvent.click(screen.getByRole("switch", { name: "Steering" }));
    await waitFor(() => {
      expect(setSettingsCall()).toMatchObject({
        modules: { steering: false },
      });
    }, { timeout: 5000 });
  });

  it("theme switch calls set_settings (not just local state) and applies the theme", async () => {
    render(<SettingsScreen />);
    await screen.findByText("Appearance");
    fireEvent.click(screen.getByRole("button", { name: "Dark" }));
    await waitFor(() => {
      expect(setSettingsCall()).toMatchObject({ theme: "dark" });
    }, { timeout: 5000 });
    expect(document.documentElement.getAttribute("data-theme")).toBe("dark");
  });

  it("language switch calls set_settings and switches the UI language", async () => {
    render(<SettingsScreen />);
    await screen.findByText("Appearance");
    fireEvent.change(screen.getByLabelText("Language"), {
      target: { value: "ur" },
    });
    await waitFor(() => {
      expect(setSettingsCall()).toMatchObject({ language: "ur" });
    }, { timeout: 5000 });
    await screen.findByText("ترتیبات");
  });

  it("Back up now calls backup_now and refreshes the generations list", async () => {
    render(<SettingsScreen />);
    await screen.findByText("Data");
    fireEvent.click(screen.getByRole("button", { name: "Back up now" }));
    await screen.findByText("Backup saved.");
    expect(mockInvoke).toHaveBeenCalledWith(
      "backup_now",
      {},
      undefined,
    );
    expect(screen.getByText("Current")).toBeInTheDocument();
  });

  it("Restore calls restore_backup with the typed path", async () => {
    render(<SettingsScreen />);
    await screen.findByText("Data");
    const pathInput = screen.getByLabelText("Backup file");
    fireEvent.change(pathInput, {
      target: { value: "/config/backups/backup-1.zip" },
    });
    // The generations list has its own Restore buttons; scope to the
    // manual path-restore row (the input's parent container).
    const manualRestore = within(
      pathInput.parentElement as HTMLElement,
    ).getByRole("button", { name: "Restore" });
    fireEvent.click(manualRestore);
    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith(
        "restore_backup",
        { path: "/config/backups/backup-1.zip" },
        undefined,
      );
    }, { timeout: 5000 });
    await screen.findByText("Backup restored.");
  });

  it("per-generation Restore calls restore_backup with the generation file", async () => {
    render(<SettingsScreen />);
    await screen.findByText("Backup 1");
    const row = screen.getByText("Backup 1").closest("li") as HTMLElement;
    fireEvent.click(within(row).getByRole("button", { name: "Restore" }));
    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith(
        "restore_backup",
        { path: "profiles.json.bak1" },
        undefined,
      );
    }, { timeout: 5000 });
    await screen.findByText("Backup restored.");
  });

  it("Open config folder calls open_config_folder", async () => {
    render(<SettingsScreen />);
    await screen.findByText("Data");
    fireEvent.click(
      screen.getByRole("button", { name: "Open config folder" }),
    );
    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith(
        "open_config_folder",
        {},
        undefined,
      );
    }, { timeout: 5000 });
  });

  it("transport save calls switch_transport with the chosen transport and address", async () => {
    render(<SettingsScreen />);
    await screen.findByLabelText("Wheel address");
    fireEvent.change(screen.getByLabelText("Wheel address"), {
      target: { value: "192.168.4.2" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Save transport" }));
    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith(
        "switch_transport",
        { transport: "wifi", address: "192.168.4.2" },
        undefined,
      );
    }, { timeout: 5000 });
    await screen.findByText("Transport saved.");
  });

  it("autostart toggle calls set_autostart", async () => {
    render(<SettingsScreen />);
    const autostartSwitch = await screen.findByRole("switch", {
      name: "Start with Windows",
    });
    fireEvent.click(autostartSwitch);
    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith(
        "set_autostart",
        { enabled: true },
        undefined,
      );
    }, { timeout: 5000 });
    await screen.findByText("Autostart setting saved.");
  });

  it("firewall rule state is wired to get_firewall_state", async () => {
    render(<SettingsScreen />);
    await screen.findByText("Firewall rule");
    await screen.findByText("No firewall rule yet.");
    expect(screen.getByText("No inbound rule for RSH.exe.")).toBeInTheDocument();
    expect(mockInvoke).toHaveBeenCalledWith(
      "get_firewall_state",
      {},
      undefined,
    );
  });

  it("get_settings not implemented: honest unavailable state", async () => {
    failGetSettingsNotImplemented = true;
    render(<SettingsScreen />);
    await screen.findByText(
      "Settings are not available yet — the backend command is not implemented.",
    );
    expect(
      screen.getByRole("switch", { name: "Steering" }),
    ).toBeDisabled();
  });
});

describe("parseSettings", () => {
  it("parses a full payload", () => {
    const parsed = parseSettings({
      modules: { steering: false, led: false },
      theme: "dark",
      language: "ur",
    });
    expect(parsed.modules.steering).toBe(false);
    expect(parsed.modules.led).toBe(false);
    expect(parsed.modules.brake).toBe(true);
    expect(parsed.theme).toBe("dark");
    expect(parsed.language).toBe("ur");
  });

  it("falls back to defaults on garbage input", () => {
    const parsed = parseSettings({
      modules: "not-an-object",
      theme: "neon",
      language: "xx",
    });
    expect(parsed.modules.steering).toBe(true);
    expect(parsed.theme).toBe("light");
    expect(parsed.language).toBe("en");
    expect(parseSettings(null).theme).toBe("light");
  });
});
