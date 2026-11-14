import { describe, it, expect, vi, beforeEach, beforeAll, afterEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import i18n from "@/lib/i18n";
import { OutputScreen } from "./index";
import en from "./strings.en.json";
import ur from "./strings.ur.json";
import hi from "./strings.hi.json";

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

function flattenKeys(value: unknown, prefix = ""): string[] {
  if (value === null || typeof value !== "object") return [prefix];
  return Object.entries(value as Record<string, unknown>).flatMap(
    ([key, child]) => flattenKeys(child, prefix ? `${prefix}.${key}` : key),
  );
}

const SNAPSHOT = {
  steeringDeg: 90,
  pedals: { accel: 0.5, brake: 0, clutch: null },
  connection: "connected",
  stats: {
    loopHz: 200,
    lossPct: 0,
    jitterMsP99: 0.5,
    outputLatencyMs: 1,
    e2eMs: 4,
  },
  activeProfileId: null,
  seq: 1,
  seqGap: 0,
  txFail: 0,
};

const BACKENDS = [
  {
    id: "hidmaestro",
    name: "HIDMaestro",
    isDefault: true,
    selected: false,
    active: false,
    connected: false,
    installed: true,
    elevationRequired: true,
    uacNote:
      "HIDMaestro runs its helper elevated for the whole session — Windows will ask (UAC) on every launch while this backend is active. vJoy needs no prompt.",
    detail: "HIDMaestro sidecar found.",
  },
  {
    id: "vjoy",
    name: "vJoy",
    isDefault: false,
    selected: true,
    active: true,
    connected: true,
    installed: true,
    elevationRequired: false,
    detail: "vJoy device 1 ready",
  },
  {
    id: "none",
    name: "None",
    isDefault: false,
    selected: false,
    active: false,
    connected: false,
    installed: true,
    elevationRequired: false,
    detail: "No output device — the wheel sends nothing to games.",
  },
];

describe("Output screen", () => {
  beforeAll(() => {
    i18n.addResourceBundle("en", "translation", en, true, true);
    i18n.addResourceBundle("ur", "translation", ur, true, true);
    i18n.addResourceBundle("hi", "translation", hi, true, true);
  });

  beforeEach(() => {
    mockInvoke.mockReset();
    mockListen.mockReset();
    for (const key of Object.keys(handlers)) delete handlers[key];
    mockInvoke.mockImplementation((cmd: string) => {
      switch (cmd) {
        case "get_output_state":
          return Promise.resolve({
            backend: "vJoy",
            connected: true,
            detail: "vJoy device 1 ready",
            enabled: true,
          });
        case "set_output_state":
          return Promise.resolve({
            backend: "vJoy",
            connected: true,
            detail: "vJoy device 1 ready",
            enabled: false,
          });
        case "list_output_backends":
          return Promise.resolve(BACKENDS);
        case "set_output_backend":
          return Promise.resolve({
            backend: "vJoy",
            connected: true,
            detail: "vJoy device 1 ready",
            enabled: true,
          });
        case "get_snapshot":
          return Promise.resolve(SNAPSHOT);
        default:
          return Promise.reject(new Error(`unknown command "${cmd}"`));
      }
    });
    mockListen.mockImplementation((event, handler) => {
      handlers[event as string] = handler as EventHandler;
      return Promise.resolve(() => {});
    });
    installTauriMock();
  });

  afterEach(() => {
    removeTauriMock();
  });

  it("shows the backend name exactly as the backend reports it", async () => {
    render(<OutputScreen />);
    // "vJoy" now appears twice: the backend card and the picker radio.
    expect((await screen.findAllByText("vJoy")).length).toBeGreaterThanOrEqual(2);
    expect(screen.getAllByText("vJoy device 1 ready").length).toBe(2);
    // "Connected" badges: the backend card and the picker.
    expect(screen.getAllByText("Connected").length).toBe(2);
    expect(screen.getByText("Output enabled")).toBeInTheDocument();
  });

  it("toggle calls set_output_state with the new value", async () => {
    render(<OutputScreen />);
    const toggle = await screen.findByRole("switch", {
      name: "Enable output",
    });
    expect(toggle).toBeChecked();
    fireEvent.click(toggle);
    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith(
        "set_output_state",
        { enabled: false },
        undefined,
      );
    });
    expect(await screen.findByText("Disabled")).toBeInTheDocument();
  });

  it("an absent enabled flag is Unknown, never inferred from connected (M5)", async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === "get_output_state") {
        // No `enabled` key: the backend has not reported it.
        return Promise.resolve({
          backend: "vJoy",
          connected: true,
          detail: "vJoy device 1 ready",
        });
      }
      if (cmd === "get_snapshot") return Promise.resolve(SNAPSHOT);
      if (cmd === "list_output_backends") return Promise.resolve(BACKENDS);
      return Promise.reject(new Error(`unknown command "${cmd}"`));
    });
    render(<OutputScreen />);
    expect(await screen.findByText("Unknown")).toBeInTheDocument();
    expect(screen.queryByText("Output enabled")).not.toBeInTheDocument();
    expect(
      screen.getByRole("switch", { name: "Enable output" }),
    ).toBeDisabled();
  });

  it("shows live steering and pedal values from the snapshot", async () => {
    render(<OutputScreen />);
    expect(await screen.findByText("90°")).toBeInTheDocument();
    expect(screen.getByText("50%")).toBeInTheDocument();
    // Clutch is null in the snapshot: no fake clutch bar, honest note.
    expect(screen.getByText(/Not present/)).toBeInTheDocument();
  });

  it("shows pressed buttons from the button-press event", async () => {
    render(<OutputScreen />);
    expect((await screen.findAllByText("vJoy")).length).toBeGreaterThanOrEqual(1);
    handlers["rsh://button-press"]?.({
      event: "rsh://button-press",
      id: 0,
      payload: { button: 3, pressed: true },
    });
    expect(await screen.findByText("#3")).toBeInTheDocument();
  });

  it("shows an honest warning when the backend is disconnected", async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === "get_output_state") {
        return Promise.resolve({
          backend: "HIDMaestro",
          connected: false,
          detail: "Driver not installed",
        });
      }
      if (cmd === "get_snapshot") return Promise.resolve(SNAPSHOT);
      return Promise.reject(new Error(`unknown command "${cmd}"`));
    });
    render(<OutputScreen />);
    expect(await screen.findByText("HIDMaestro")).toBeInTheDocument();
    expect(screen.getByText("Disconnected")).toBeInTheDocument();
    expect(
      screen.getByText(/No output backend is available/i),
    ).toBeInTheDocument();
    // Toggle is disabled while no backend is connected.
    expect(
      screen.getByRole("switch", { name: "Enable output" }),
    ).toBeDisabled();
  });

  it("shows the backend picker with honest per-backend status", async () => {
    render(<OutputScreen />);
    expect(await screen.findByText("Backend selection")).toBeInTheDocument();
    expect(screen.getByText("HIDMaestro")).toBeInTheDocument();
    expect(screen.getByText("Default")).toBeInTheDocument();
    expect(screen.getByText("HIDMaestro sidecar found.")).toBeInTheDocument();
    // The UAC-every-launch caveat is visible on the page.
    expect(
      screen.getByText(/Windows will ask \(UAC\) on every launch/),
    ).toBeInTheDocument();
    // vJoy is installed here: no install link, no "Not installed" badge.
    expect(screen.queryByText("Install vJoy")).not.toBeInTheDocument();
    expect(screen.queryByText("Not installed")).not.toBeInTheDocument();
  });

  it("selecting a backend calls set_output_backend", async () => {
    render(<OutputScreen />);
    const radio = await screen.findByRole("radio", { name: /HIDMaestro/ });
    fireEvent.click(radio);
    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith(
        "set_output_backend",
        { backend: "hidmaestro" },
        undefined,
      );
    });
  });

  it("shows the fallback banner when the selected backend is not live", async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === "list_output_backends") {
        return Promise.resolve([
          { ...BACKENDS[0], selected: true, active: false },
          { ...BACKENDS[1], selected: false, active: true },
          BACKENDS[2],
        ]);
      }
      if (cmd === "get_output_state") {
        return Promise.resolve({
          backend: "vJoy",
          connected: true,
          detail:
            "vJoy device 1 — fallback backend. HIDMaestro could not connect, fell back to vJoy. Reason: HIDMaestro sidecar not found.",
          enabled: true,
        });
      }
      if (cmd === "get_snapshot") return Promise.resolve(SNAPSHOT);
      return Promise.reject(new Error(`unknown command "${cmd}"`));
    });
    render(<OutputScreen />);
    expect(
      await screen.findByText(/The selected backend could not connect/),
    ).toBeInTheDocument();
    // The reason shows twice: the backend card detail and the banner.
    expect(screen.getAllByText(/HIDMaestro sidecar not found/).length).toBe(2);
  });

  it("shows the vJoy install link when vJoy is missing", async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === "list_output_backends") {
        return Promise.resolve([
          BACKENDS[0],
          {
            ...BACKENDS[1],
            installed: false,
            detail: "vJoy not found — install vJoy 2.2.2.0.",
            downloadUrl: "https://simux.brunner-innovation.swiss/guide/vjoy/",
          },
          BACKENDS[2],
        ]);
      }
      if (cmd === "get_output_state") {
        return Promise.resolve({
          backend: "HIDMaestro",
          connected: false,
          detail: "HIDMaestro sidecar found.",
        });
      }
      if (cmd === "get_snapshot") return Promise.resolve(SNAPSHOT);
      return Promise.reject(new Error(`unknown command "${cmd}"`));
    });
    render(<OutputScreen />);
    const link = await screen.findByText("Install vJoy");
    expect(link).toBeInTheDocument();
    expect(link.closest("a")).toHaveAttribute(
      "href",
      "https://simux.brunner-innovation.swiss/guide/vjoy/",
    );
    expect(screen.getByText("Not installed")).toBeInTheDocument();
  });

  it("shows an honest not-available state when get_output_state is not implemented", async () => {
    mockInvoke.mockImplementation((cmd: string) =>
      Promise.reject(new Error(`unknown command "${cmd}"`)),
    );
    render(<OutputScreen />);
    expect(
      await screen.findByText(/not available yet/i),
    ).toBeInTheDocument();
  });

  it("strings have key parity across en/ur/hi", () => {
    expect(flattenKeys(en).sort()).toEqual(flattenKeys(ur).sort());
    expect(flattenKeys(en).sort()).toEqual(flattenKeys(hi).sort());
  });
});
