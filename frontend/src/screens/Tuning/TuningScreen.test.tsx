import { describe, it, expect, vi, beforeAll, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent, waitFor, within } from "@testing-library/react";
import i18n, { changeLanguage } from "../../lib/i18n";
import { TuningScreen } from "./TuningScreen";
import { defaultTuning } from "./tuning";
import type { EngineSnapshot, TuningSettings } from "../../lib/tauri";
import enStrings from "./strings.en.json";
import urStrings from "./strings.ur.json";
import hiStrings from "./strings.hi.json";

// jsdom has no ResizeObserver; Radix Slider measures its track with one.
// A no-op stub is enough here — tests drive sliders by keyboard, which
// does not need geometry. (Real browsers and WebView2 always provide it.)
if (typeof globalThis.ResizeObserver === "undefined") {
  globalThis.ResizeObserver = class {
    constructor(_callback: ResizeObserverCallback) {}
    observe(_target: Element): void {}
    unobserve(_target: Element): void {}
    disconnect(): void {}
  };
}

// Mocks live at the window.__TAURI__ boundary only (same seam as
// src/lib/tauri.test.ts): invoke through __TAURI_INTERNALS__, events
// through @tauri-apps/api/event's listen.
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));

import { listen } from "@tauri-apps/api/event";

const mockListen = vi.mocked(listen);
const mockInvoke = vi.fn();

type SnapshotHandler = (event: {
  event: string;
  id: number;
  payload: unknown;
}) => void;

let tuningStore: Record<string, TuningSettings> = {};
let settingsResponse: Record<string, unknown> = {};
let commandFailures: Record<string, Error> = {};
let deferGetTuning = false;
let resolveDeferredTuning: ((value: TuningSettings) => void) | null = null;

function makeSnapshot(
  overrides: Partial<EngineSnapshot> = {},
): EngineSnapshot {
  return {
    steeringDeg: 0,
    pedals: { accel: 0, brake: 0, clutch: null },
    connection: "connected",
    stats: {
      loopHz: 200,
      lossPct: 0,
      jitterMsP99: 0,
      outputLatencyMs: 0,
      e2eMs: 0,
    },
    activeProfileId: null,
    seq: 1,
    ...overrides,
  };
}

function fireSnapshot(snapshot: EngineSnapshot) {
  for (const call of mockListen.mock.calls) {
    (call[1] as SnapshotHandler)({
      event: "rsh://snapshot",
      id: 0,
      payload: snapshot,
    });
  }
}

function setTuningCalls() {
  return mockInvoke.mock.calls.filter((call) => call[0] === "set_tuning");
}

/** Card buttons are found by their explicit accessible name ("Label: …"). */
function cardButton(label: string) {
  return screen.getByRole("button", {
    name: new RegExp(`^${label}:`),
  });
}

function flattenKeys(obj: Record<string, unknown>, prefix = ""): string[] {
  const keys: string[] = [];
  for (const [key, value] of Object.entries(obj)) {
    const full = prefix ? `${prefix}.${key}` : key;
    if (value !== null && typeof value === "object") {
      keys.push(...flattenKeys(value as Record<string, unknown>, full));
    } else {
      keys.push(full);
    }
  }
  return keys.sort();
}

beforeAll(() => {
  // The coordinator merges these into src/locales/ at the Phase E gate;
  // until then the tests register them here, exactly as the task specifies.
  i18n.addResourceBundle("en", "translation", enStrings, true, true);
  i18n.addResourceBundle("ur", "translation", urStrings, true, true);
  i18n.addResourceBundle("hi", "translation", hiStrings, true, true);
});

beforeEach(() => {
  tuningStore = {};
  settingsResponse = {
    modules: {
      steering: true,
      accel: true,
      brake: true,
      clutch: true,
      handbrake: true,
      shifter: true,
      horn: true,
      buttons: true,
      encoders: true,
      led: true,
    },
  };
  commandFailures = {};
  deferGetTuning = false;
  resolveDeferredTuning = null;
  mockInvoke.mockReset();
  mockListen.mockReset();
  mockListen.mockResolvedValue(() => {});
  mockInvoke.mockImplementation((cmd: string, args?: Record<string, unknown>) => {
    const failure = commandFailures[cmd];
    if (failure) return Promise.reject(failure);
    if (cmd === "get_settings") return Promise.resolve(settingsResponse);
    if (cmd === "get_tuning") {
      const moduleId = (args as { module: string }).module;
      if (deferGetTuning) {
        return new Promise<TuningSettings>((resolve) => {
          resolveDeferredTuning = resolve;
        });
      }
      return Promise.resolve(tuningStore[moduleId] ?? defaultTuning(moduleId));
    }
    if (cmd === "set_tuning") {
      const tuning = args as unknown as TuningSettings;
      tuningStore[tuning.module] = tuning;
      return Promise.resolve(tuning);
    }
    return Promise.reject(new Error(`unknown command "${cmd}"`));
  });
  (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {
    invoke: mockInvoke,
  };
});

afterEach(async () => {
  delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
  await changeLanguage("en");
});

describe("TuningScreen cards", () => {
  it("renders all 10 module cards with On state badges", async () => {
    render(<TuningScreen />);
    expect(
      screen.getByRole("heading", { name: "Tuning" }),
    ).toBeInTheDocument();
    // Honest note: tuning saves to the profile but does not reach the
    // wheel output yet (engine runs its default plan — Phase G cond. 14).
    expect(
      screen.getByText(
        "Tuning is saved to the active profile. The engine still runs its default plan, so tuning does not change the wheel output yet.",
      ),
    ).toBeInTheDocument();
    const cards: Array<[string, string]> = [
      ["Steering", "Steering wheel response"],
      ["Accelerator", "Accelerator pedal response"],
      ["Brake", "Brake pedal response"],
      ["Clutch", "Clutch pedal response"],
      ["Handbrake", "Handbrake response"],
      ["Shifter", "Gear shifter response"],
      ["Horn", "Horn button response"],
      ["Wheel Buttons", "Wheel button response"],
      ["Encoders", "Rotary encoder response"],
      ["Status LED", "Status LED brightness response"],
    ];
    for (const [label] of cards) {
      expect(cardButton(label)).toBeInTheDocument();
    }
    await waitFor(() =>
      expect(screen.getAllByText("On")).toHaveLength(10),
    );
  });

  it("shows Unknown state when get_settings is not implemented", async () => {
    commandFailures["get_settings"] = new Error(
      'unknown command "get_settings"',
    );
    render(<TuningScreen />);
    await waitFor(() =>
      expect(screen.getAllByText("Unknown")).toHaveLength(10),
    );
    // The panel still works: unknown state never blocks tuning.
    fireEvent.click(cardButton("Steering"));
    expect(
      await screen.findByRole("slider", { name: "Deadzone" }),
    ).toBeInTheDocument();
  });

  it("shows the live input mini value on cards with a snapshot channel", async () => {
    render(<TuningScreen />);
    fireSnapshot(makeSnapshot({ steeringDeg: 0 }));
    expect(await screen.findByText("Live input: 50%")).toBeInTheDocument();
  });

  it("opens exactly one panel at a time, and closes on a second tap", async () => {
    render(<TuningScreen />);
    fireEvent.click(cardButton("Steering"));
    expect(
      await screen.findByRole("heading", { name: "Steering" }),
    ).toBeInTheDocument();
    expect(screen.getAllByRole("slider")).toHaveLength(5);

    fireEvent.click(cardButton("Brake"));
    expect(
      await screen.findByRole("heading", { name: "Brake" }),
    ).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "Steering" })).toBeNull();
    expect(screen.getAllByRole("slider")).toHaveLength(5);

    fireEvent.click(cardButton("Brake"));
    await waitFor(() =>
      expect(screen.queryByRole("heading", { name: "Brake" })).toBeNull(),
    );
    expect(screen.queryAllByRole("slider")).toHaveLength(0);
  });

  it("the panel close button closes the panel", async () => {
    render(<TuningScreen />);
    fireEvent.click(cardButton("Steering"));
    expect(
      await screen.findByRole("slider", { name: "Deadzone" }),
    ).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Close panel" }));
    await waitFor(() =>
      expect(screen.queryByRole("slider", { name: "Deadzone" })).toBeNull(),
    );
  });

  it("an off module opens a read-only panel", async () => {
    settingsResponse = { modules: { steering: true, brake: false } };
    render(<TuningScreen />);
    expect(await screen.findByText("Off")).toBeInTheDocument();
    fireEvent.click(cardButton("Brake"));
    expect(
      await screen.findByText(
        "This module is off. Turn it on in Settings to tune it.",
      ),
    ).toBeInTheDocument();
    const deadzone = await screen.findByRole("slider", { name: "Deadzone" });
    expect(deadzone).toHaveAttribute("aria-disabled", "true");
    expect(
      screen.getByRole("button", { name: /Reset to defaults/ }),
    ).toBeDisabled();
  });
});

describe("TuningPanel load / save", () => {
  it("shows a loading state while get_tuning is pending", async () => {
    deferGetTuning = true;
    render(<TuningScreen />);
    fireEvent.click(cardButton("Steering"));
    expect(await screen.findByText("Loading…")).toBeInTheDocument();
    expect(screen.queryByRole("slider")).toBeNull();
    resolveDeferredTuning?.(defaultTuning("steering"));
    expect(
      await screen.findByRole("slider", { name: "Deadzone" }),
    ).toBeInTheDocument();
  });

  it("shows an honest NOT_IMPLEMENTED state when get_tuning is missing", async () => {
    commandFailures["get_tuning"] = new Error('unknown command "get_tuning"');
    render(<TuningScreen />);
    fireEvent.click(cardButton("Steering"));
    expect(
      await screen.findByText("Tuning is not available yet."),
    ).toBeInTheDocument();
    expect(
      screen.getByText(
        "The backend command get_tuning is not implemented yet.",
      ),
    ).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Retry" })).toBeInTheDocument();
    expect(screen.queryByRole("slider")).toBeNull();
  });

  it("shows a backend error with Retry, and Retry recovers", async () => {
    commandFailures["get_tuning"] = new Error("transport timeout");
    render(<TuningScreen />);
    fireEvent.click(cardButton("Steering"));
    expect(
      await screen.findByText("Could not load tuning."),
    ).toBeInTheDocument();
    expect(screen.getByText(/transport timeout/)).toBeInTheDocument();
    delete commandFailures["get_tuning"];
    fireEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(
      await screen.findByRole("slider", { name: "Deadzone" }),
    ).toBeInTheDocument();
  });

  it("slider edits call set_tuning with the changed value", async () => {
    render(<TuningScreen />);
    fireEvent.click(cardButton("Steering"));
    const deadzone = await screen.findByRole("slider", { name: "Deadzone" });
    expect(deadzone).toHaveAttribute("aria-valuenow", "0");
    for (let i = 0; i < 5; i++) {
      fireEvent.keyDown(deadzone, { key: "ArrowRight" });
    }
    expect(
      screen.getByRole("slider", { name: "Deadzone" }),
    ).toHaveAttribute("aria-valuenow", "5");
    await waitFor(
      () => {
        const calls = setTuningCalls();
        expect(calls.length).toBeGreaterThan(0);
        expect(calls[calls.length - 1]?.[1]).toMatchObject({
          module: "steering",
          deadzone: 5,
        });
      },
      { timeout: 3000 },
    );
    expect(await screen.findByText("Saved")).toBeInTheDocument();
  });

  it("edits round-trip: save then reload shows the saved values", async () => {
    render(<TuningScreen />);
    fireEvent.click(cardButton("Steering"));
    const deadzone = await screen.findByRole("slider", { name: "Deadzone" });
    for (let i = 0; i < 5; i++) {
      fireEvent.keyDown(deadzone, { key: "ArrowRight" });
    }
    await waitFor(
      () => expect(setTuningCalls().length).toBeGreaterThan(0),
      { timeout: 3000 },
    );
    expect(await screen.findByText("Saved")).toBeInTheDocument();

    fireEvent.click(cardButton("Brake"));
    expect(
      await screen.findByRole("heading", { name: "Brake" }),
    ).toBeInTheDocument();
    fireEvent.click(cardButton("Steering"));
    const reloaded = await screen.findByRole("slider", {
      name: "Deadzone",
    });
    expect(reloaded).toHaveAttribute("aria-valuenow", "5");
  });

  it("Reset to defaults restores factory values and saves them", async () => {
    tuningStore["steering"] = {
      module: "steering",
      deadzone: 12,
      curve: 1.5,
      minMv: 200,
      maxMv: 3000,
      smoothing: 40,
    };
    render(<TuningScreen />);
    fireEvent.click(cardButton("Steering"));
    const deadzone = await screen.findByRole("slider", { name: "Deadzone" });
    expect(deadzone).toHaveAttribute("aria-valuenow", "12");
    fireEvent.click(screen.getByRole("button", { name: /Reset to defaults/ }));
    expect(
      screen.getByRole("slider", { name: "Deadzone" }),
    ).toHaveAttribute("aria-valuenow", "0");
    await waitFor(
      () => {
        const calls = setTuningCalls();
        expect(calls.length).toBeGreaterThan(0);
        expect(calls[calls.length - 1]?.[1]).toMatchObject({
          module: "steering",
          deadzone: 0,
          curve: 1,
          minMv: 0,
          maxMv: 3300,
          smoothing: 0,
        });
      },
      { timeout: 3000 },
    );
  });

  it("a failed save shows an error and keeps the draft", async () => {
    commandFailures["set_tuning"] = new Error("disk full");
    render(<TuningScreen />);
    fireEvent.click(cardButton("Steering"));
    const deadzone = await screen.findByRole("slider", { name: "Deadzone" });
    fireEvent.keyDown(deadzone, { key: "ArrowRight" });
    expect(
      await screen.findByText(/Could not save tuning\./),
    ).toBeInTheDocument();
    expect(screen.getByText(/disk full/)).toBeInTheDocument();
    expect(
      screen.getByRole("slider", { name: "Deadzone" }),
    ).toHaveAttribute("aria-valuenow", "1");
  });

  it("clamps endpoints to the 660 mV span guard and says so", async () => {
    tuningStore["steering"] = {
      module: "steering",
      deadzone: 0,
      curve: 1,
      minMv: 2600,
      maxMv: 3300,
      smoothing: 0,
    };
    render(<TuningScreen />);
    fireEvent.click(cardButton("Steering"));
    const minSlider = await screen.findByRole("slider", {
      name: /Minimum/,
    });
    for (let i = 0; i < 5; i++) {
      fireEvent.keyDown(minSlider, { key: "ArrowRight" });
    }
    expect(
      await screen.findByText(
        "Endpoints must span at least 660 mV. The value was clamped.",
      ),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("slider", { name: /Minimum/ }),
    ).toHaveAttribute("aria-valuenow", "2640");
    await waitFor(
      () => {
        const calls = setTuningCalls();
        expect(calls.length).toBeGreaterThan(0);
        expect(calls[calls.length - 1]?.[1]).toMatchObject({
          minMv: 2640,
          maxMv: 3300,
        });
      },
      { timeout: 3000 },
    );
  });
});

describe("TuningPanel live curve", () => {
  it("renders the curve from snapshot data and shows the live input", async () => {
    render(<TuningScreen />);
    fireSnapshot(makeSnapshot({ steeringDeg: 0 }));
    expect(await screen.findByText("Live input: 50%")).toBeInTheDocument();

    fireEvent.click(cardButton("Steering"));
    await screen.findByRole("slider", { name: "Deadzone" });
    fireSnapshot(makeSnapshot({ steeringDeg: 450 }));
    // The panel live readout tracks the feed (scoped to the panel: the
    // card mini value has its own 250 ms throttle and may lag this event).
    const heading = screen.getByRole("heading", { name: "Steering" });
    const panelEl = heading.closest('[aria-label="Steering tuning"]');
    expect(panelEl).not.toBeNull();
    expect(
      await within(panelEl as HTMLElement).findByText("Live input: 100%"),
    ).toBeInTheDocument();
    // The curve itself renders (SVG fallback in jsdom) with 61 points.
    await waitFor(() =>
      expect(document.querySelector("polyline")).not.toBeNull(),
    );
    const polyline = document.querySelector("polyline");
    expect(polyline).not.toBeNull();
    expect(
      polyline?.getAttribute("points")?.trim().split(/\s+/).length,
    ).toBe(61);
  });

  it("modules without a snapshot channel show no live input", async () => {
    render(<TuningScreen />);
    fireEvent.click(
      cardButton("Status LED"),
    );
    await screen.findByRole("slider", { name: "Deadzone" });
    expect(
      screen.getByText("No live input for this module."),
    ).toBeInTheDocument();
  });
});

describe("Tuning strings", () => {
  it("has identical key sets in en, ur and hi", () => {
    const enKeys = flattenKeys(enStrings as Record<string, unknown>);
    expect(enKeys.length).toBeGreaterThan(0);
    expect(flattenKeys(urStrings as Record<string, unknown>)).toEqual(enKeys);
    expect(flattenKeys(hiStrings as Record<string, unknown>)).toEqual(enKeys);
  });

  it("renders the Urdu title after switching language", async () => {
    await changeLanguage("ur");
    render(<TuningScreen />);
    expect(
      screen.getByRole("heading", { name: "ٹیوننگ" }),
    ).toBeInTheDocument();
  });
});
