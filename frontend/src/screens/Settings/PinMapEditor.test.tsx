import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import i18n from "@/lib/i18n";
import { applyTheme } from "@/lib/theme";
import { PinMapEditor } from "./PinMapEditor";
import type { PinMapValidation } from "@/lib/tauri";
import enStrings from "./strings.en.json";
import urStrings from "./strings.ur.json";
import hiStrings from "./strings.hi.json";

// Mocks live at the window.__TAURI__ boundary only (same seam as
// src/lib/tauri.test.ts). The validator itself is never re-implemented
// here — these doubles stand in for the Rust `validate_pin_map` command.
const mockInvoke = vi.fn();

function installTauriMock() {
  (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {
    invoke: mockInvoke,
  };
}

function removeTauriMock() {
  delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
}

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

const VALID: PinMapValidation = { valid: true, errors: [], warnings: [] };

let validationResult: PinMapValidation = VALID;
let failValidateNotImplemented = false;
let failPinMapNotImplemented = false;

describe("PinMapEditor", () => {
  beforeEach(() => {
    i18n.addResourceBundle("en", "translation", enStrings, true, true);
    i18n.addResourceBundle("ur", "translation", urStrings, true, true);
    i18n.addResourceBundle("hi", "translation", hiStrings, true, true);
    validationResult = VALID;
    failValidateNotImplemented = false;
    failPinMapNotImplemented = false;
    mockInvoke.mockReset();
    mockInvoke.mockImplementation((command: string, _args?: unknown) => {
        switch (command) {
          case "get_pin_map":
            if (failPinMapNotImplemented) {
              return Promise.reject(
                new Error('unknown command "get_pin_map"'),
              );
            }
            return Promise.resolve(PIN_MAP);
          case "validate_pin_map":
            if (failValidateNotImplemented) {
              return Promise.reject(
                new Error('unknown command "validate_pin_map"'),
              );
            }
            return Promise.resolve(validationResult);
          case "set_pin_map":
            return Promise.resolve({
              valid: true,
              errors: [],
              warnings: [],
            });
          case "confirm_config":
            return Promise.resolve("clean");
          case "rollback_config":
            return Promise.resolve("rolled_back");
          default:
            return Promise.reject(
              new Error(`unknown command "${command}"`),
            );
        }
      },
    );
    installTauriMock();
  });

  afterEach(() => {
    removeTauriMock();
    applyTheme("light");
  });

  it("valid map: no errors and Apply is enabled", async () => {
    render(<PinMapEditor />);
    await screen.findByText("Pin map is valid.", {}, { timeout: 5000 });
    expect(
      screen.getByRole("button", { name: "Apply pin map" }),
    ).toBeEnabled();
    expect(screen.queryByText("Validation errors")).not.toBeInTheDocument();
    expect(mockInvoke).toHaveBeenCalledWith(
      "validate_pin_map",
      expect.objectContaining({
        pins: expect.arrayContaining([
          { function: "steering", pin: 32 },
        ]),
      }),
      undefined,
    );
  });

  it("invalid map: errors are listed and Apply is disabled", async () => {
    validationResult = {
      valid: false,
      errors: [
        "Steering pin GPIO 32 is already assigned to brake.",
        "GPIO 32 is used twice.",
      ],
      warnings: [],
    };
    render(<PinMapEditor />);
    await screen.findAllByText(
      "Steering pin GPIO 32 is already assigned to brake.",
      {},
      { timeout: 5000 },
    );
    expect(screen.getByText("Validation errors")).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Apply pin map" }),
    ).toBeDisabled();
    expect(mockInvoke).not.toHaveBeenCalledWith(
      "set_pin_map",
      expect.anything(),
      undefined,
    );
  });

  it("warnings are shown inline per pin and in the warnings list", async () => {
    validationResult = {
      valid: true,
      errors: [],
      warnings: ["Steering pin GPIO 32 is a strapping pin."],
    };
    render(<PinMapEditor />);
    await screen.findByText("Warnings", {}, { timeout: 5000 });
    expect(
      screen.getAllByText("Steering pin GPIO 32 is a strapping pin.").length,
    ).toBeGreaterThanOrEqual(1);
    expect(
      screen.getByRole("button", { name: "Apply pin map" }),
    ).toBeEnabled();
  });

  it("changing a pin re-validates the draft", async () => {
    render(<PinMapEditor />);
    await screen.findByText("Pin map is valid.", {}, { timeout: 5000 });
    const callsBefore = mockInvoke.mock.calls.filter(
      (c) => c[0] === "validate_pin_map",
    ).length;
    fireEvent.change(
      screen.getByLabelText("GPIO pin for Steering"),
      { target: { value: "4" } },
    );
    await waitFor(
      () => {
        const calls = mockInvoke.mock.calls.filter(
          (c) => c[0] === "validate_pin_map",
        );
        expect(calls.length).toBeGreaterThan(callsBefore);
        const last = calls[calls.length - 1][1] as {
          pins: { function: string; pin: number }[];
        };
        expect(last.pins).toContainEqual({
          function: "steering",
          pin: 4,
        });
      },
      { timeout: 5000 },
    );
  });

  it("validate_pin_map not implemented: honest validation failure, Apply disabled", async () => {
    failValidateNotImplemented = true;
    render(<PinMapEditor />);
    await screen.findByText(
      "Could not validate the pin map.",
      {},
      { timeout: 5000 },
    );
    expect(
      screen.getByRole("button", { name: "Apply pin map" }),
    ).toBeDisabled();
  });

  it("get_pin_map not implemented: editor starts from an empty draft, validation still works", async () => {
    failPinMapNotImplemented = true;
    render(<PinMapEditor />);
    await screen.findByText(
      "Could not read the board's pin map — starting from an empty map. Validation works; applying to the board is not available yet.",
      {},
      { timeout: 5000 },
    );
    // The editor stays usable: pin selects render and the (empty)
    // draft validates, instead of a dead screen.
    expect(
      screen.getByRole("button", { name: "Apply pin map" }),
    ).toBeInTheDocument();
    await screen.findByText("Pin map is valid.", {}, { timeout: 5000 });
  });

  it("Apply sends the map and shows the confirm bar; Confirm confirms it", async () => {
    render(<PinMapEditor />);
    await screen.findByText("Pin map is valid.", {}, { timeout: 5000 });
    fireEvent.click(screen.getByRole("button", { name: "Apply pin map" }));
    await screen.findByText(
      "Pin map applied. Confirm within 30 seconds or the previous map is restored.",
    );
    expect(mockInvoke).toHaveBeenCalledWith(
      "set_pin_map",
      expect.objectContaining({
        pins: expect.arrayContaining([
          { function: "steering", pin: 32 },
        ]),
      }),
      undefined,
    );
    fireEvent.click(screen.getByRole("button", { name: "Confirm" }));
    await screen.findByText("Pin map confirmed.");
    expect(mockInvoke).toHaveBeenCalledWith(
      "confirm_config",
      {},
      undefined,
    );
  });

  it("Roll back calls rollback_config and reloads the map", async () => {
    render(<PinMapEditor />);
    await screen.findByText("Pin map is valid.", {}, { timeout: 5000 });
    fireEvent.click(screen.getByRole("button", { name: "Apply pin map" }));
    await screen.findByRole("button", { name: "Roll back" });
    fireEvent.click(screen.getByRole("button", { name: "Roll back" }));
    await screen.findByText("Pin map rolled back.");
    expect(mockInvoke).toHaveBeenCalledWith(
      "rollback_config",
      {},
      undefined,
    );
    expect(
      mockInvoke.mock.calls.filter((c) => c[0] === "get_pin_map").length,
    ).toBeGreaterThanOrEqual(2);
  });
});
