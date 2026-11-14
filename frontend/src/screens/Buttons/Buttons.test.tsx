import { describe, it, expect, vi, beforeEach, beforeAll, afterEach } from "vitest";
import { render, screen, fireEvent, waitFor, within } from "@testing-library/react";
import i18n from "@/lib/i18n";
import { ButtonsScreen } from "./index";
import { parseBindings, withBinding } from "./bindings";
import en from "./strings.en.json";
import ur from "./strings.ur.json";
import hi from "./strings.hi.json";

// Mocks live at the window.__TAURI__ boundary only (same seam as
// src/lib/tauri.test.ts); no fake data in production code.
const mockInvoke = vi.fn();

function installTauriMock() {
  (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {
    invoke: mockInvoke,
  };
}

function removeTauriMock() {
  delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
}

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(),
}));

import { listen } from "@tauri-apps/api/event";

const mockListen = vi.mocked(listen);

type EventHandler = (event: {
  event: string;
  id: number;
  payload: unknown;
}) => void;

const handlers: Record<string, EventHandler> = {};

function buttonPress(button: number, pressed: boolean) {
  handlers["rsh://button-press"]?.({
    event: "rsh://button-press",
    id: 0,
    payload: { button, pressed },
  });
}

function flattenKeys(value: unknown, prefix = ""): string[] {
  if (value === null || typeof value !== "object") return [prefix];
  return Object.entries(value as Record<string, unknown>).flatMap(
    ([key, child]) => flattenKeys(child, prefix ? `${prefix}.${key}` : key),
  );
}

const CONFIG_WITH_BINDINGS = {
  buttons: { "1": "horn", "5": "shifter_up", "65": "gear_1" },
};

/** Backend config-transaction state modelled by the mock. */
let mockConfigState = "clean";

function mockInvokeImpl(cmd: string) {
  switch (cmd) {
    case "get_config":
      return Promise.resolve({ ...CONFIG_WITH_BINDINGS });
    case "get_config_state":
      // Models the backend transaction record: stage -> staged,
      // commit -> pending_confirm, confirm/rollback -> clean.
      return Promise.resolve(mockConfigState);
    case "stage_config":
      mockConfigState = "staged";
      return Promise.resolve("staged");
    case "commit_config":
      mockConfigState = "pending_confirm";
      return Promise.resolve("pending_confirm");
    case "confirm_config":
      mockConfigState = "clean";
      return Promise.resolve("clean");
    case "rollback_config":
      mockConfigState = "rolled_back";
      return Promise.resolve("rolled_back");
    default:
      return Promise.reject(new Error(`unknown command "${cmd}"`));
  }
}

describe("Buttons screen", () => {
  beforeAll(() => {
    i18n.addResourceBundle("en", "translation", en, true, true);
    i18n.addResourceBundle("ur", "translation", ur, true, true);
    i18n.addResourceBundle("hi", "translation", hi, true, true);
  });

  beforeEach(() => {
    mockConfigState = "clean";
    mockInvoke.mockReset();
    mockListen.mockReset();
    for (const key of Object.keys(handlers)) delete handlers[key];
    mockInvoke.mockImplementation(mockInvokeImpl);
    mockListen.mockImplementation((event, handler) => {
      handlers[event as string] = handler as EventHandler;
      return Promise.resolve(() => {});
    });
    installTauriMock();
  });

  afterEach(() => {
    removeTauriMock();
  });

  it("renders bindings from the mocked config and unbound buttons honestly", async () => {
    render(<ButtonsScreen />);
    expect(await screen.findByText("Horn")).toBeInTheDocument();
    expect(screen.getByText("Shifter up")).toBeInTheDocument();
    expect(screen.getByText("Gear 1")).toBeInTheDocument();
    // Button 2 has no binding: shown as unbound, never invented.
    expect(screen.getAllByText("Unbound").length).toBeGreaterThan(0);
    expect(screen.getByText("Physical buttons")).toBeInTheDocument();
    expect(screen.getByText("Encoder buttons")).toBeInTheDocument();
    expect(screen.getByText("Encoder 1 clockwise")).toBeInTheDocument();
  });

  it("rebind flow saves through stage -> commit -> confirm", async () => {
    render(<ButtonsScreen />);
    await screen.findByText("Horn");
    fireEvent.click(screen.getByRole("button", { name: "Rebind button 1" }));
    fireEvent.click(screen.getByRole("radio", { name: "Handbrake" }));
    fireEvent.click(screen.getByRole("button", { name: "Save binding" }));
    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith(
        "stage_config",
        expect.objectContaining({
          config: expect.objectContaining({
            buttons: { "1": "handbrake", "5": "shifter_up", "65": "gear_1" },
          }),
        }),
        undefined,
      );
    });
    const called = mockInvoke.mock.calls.map((c) => c[0] as string);
    expect(called).toContain("commit_config");
    expect(called).toContain("confirm_config");
    await waitFor(() => {
      expect(screen.getByText("Handbrake")).toBeInTheDocument();
    });
  });

  it("cancel closes the rebind dialog without saving", async () => {
    render(<ButtonsScreen />);
    await screen.findByText("Horn");
    fireEvent.click(screen.getByRole("button", { name: "Rebind button 1" }));
    fireEvent.click(screen.getByRole("radio", { name: "Handbrake" }));
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    await waitFor(() => {
      expect(
        screen.queryByRole("button", { name: "Save binding" }),
      ).not.toBeInTheDocument();
    });
    expect(mockInvoke).not.toHaveBeenCalledWith(
      "stage_config",
      expect.anything(),
      undefined,
    );
  });

  it("press-to-capture switches the rebind target to the pressed button", async () => {
    render(<ButtonsScreen />);
    await screen.findByText("Horn");
    fireEvent.click(screen.getByRole("button", { name: "Rebind button 2" }));
    expect(screen.getByText("Rebind button 2")).toBeInTheDocument();
    fireEvent.click(
      screen.getByRole("button", { name: "Press a button on the wheel" }),
    );
    buttonPress(7, true);
    await waitFor(() => {
      expect(screen.getByText("Rebind button 7")).toBeInTheDocument();
    });
  });

  it("highlights a pressed button live from the button-press event", async () => {
    render(<ButtonsScreen />);
    await screen.findByText("Horn");
    const cell = screen.getByRole("button", { name: "Rebind button 1" });
    expect(cell).toHaveAttribute("aria-pressed", "false");
    buttonPress(1, true);
    await waitFor(() => {
      expect(
        screen.getByRole("button", { name: "Rebind button 1" }),
      ).toHaveAttribute("aria-pressed", "true");
    });
    buttonPress(1, false);
    await waitFor(() => {
      expect(
        screen.getByRole("button", { name: "Rebind button 1" }),
      ).toHaveAttribute("aria-pressed", "false");
    });
  });

  it("shows an honest not-available state when get_config is not implemented", async () => {
    mockInvoke.mockImplementation((cmd: string) =>
      Promise.reject(new Error(`unknown command "${cmd}"`)),
    );
    render(<ButtonsScreen />);
    expect(
      await screen.findByText(/not available yet/i),
    ).toBeInTheDocument();
  });

  it("strings have key parity across en/ur/hi", () => {
    expect(flattenKeys(en).sort()).toEqual(flattenKeys(ur).sort());
    expect(flattenKeys(en).sort()).toEqual(flattenKeys(hi).sort());
  });

  it("dialog footer buttons are reachable", async () => {
    render(<ButtonsScreen />);
    await screen.findByText("Horn");
    fireEvent.click(screen.getByRole("button", { name: "Rebind button 5" }));
    const dialog = await screen.findByRole("dialog");
    expect(
      within(dialog).getByRole("button", { name: "Save binding" }),
    ).toBeInTheDocument();
  });
});

describe("bindings helpers", () => {
  it("parseBindings ignores corrupt entries instead of inventing bindings", () => {
    expect(
      parseBindings({
        buttons: {
          "1": "horn",
          nope: "horn",
          "2": 42,
          "3": "",
          "999": "horn",
        },
      }),
    ).toEqual({ 1: "horn", 999: "horn" });
    expect(parseBindings({})).toEqual({});
    expect(parseBindings({ buttons: "horn" })).toEqual({});
  });

  it("withBinding sets and removes bindings", () => {
    const config = { buttons: { "1": "horn" }, other: true };
    expect(withBinding(config, 2, "handbrake")).toEqual({
      buttons: { "1": "horn", "2": "handbrake" },
      other: true,
    });
    expect(withBinding(config, 1, null)).toEqual({
      buttons: {},
      other: true,
    });
  });
});
