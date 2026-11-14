import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent, within, act } from "@testing-library/react";
import i18n from "@/lib/i18n";
import { toScreenDefinition } from "@/lib/screens";
import {
  component,
  icon,
  label,
  path,
  SetupScreen,
} from "./index";
import { PairingStep, groupPassword } from "./PairingStep";
import { FirmwareStep, parseVersion, compareVersions } from "./FirmwareStep";
import enStrings from "./strings.en.json";
import urStrings from "./strings.ur.json";
import hiStrings from "./strings.hi.json";

// The mock lives at the window.__TAURI__ boundary, exactly like
// src/lib/tauri.test.ts: @tauri-apps/api/core's invoke() calls
// window.__TAURI_INTERNALS__.invoke at call time. No fake data is
// shipped in the components themselves — these doubles live only here.
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

// The screen registers its own strings on import (see index.tsx); the
// coordinator merges them into src/locales/ at the Phase E gate. Register
// them explicitly here too so the tests assert the shipped bundles.
let validateResult: {
  valid: boolean;
  version: string | null;
  signatureValid: boolean;
  errors: string[];
} = { valid: true, version: "4.0.2", signatureValid: true, errors: [] };

function defaultInvoke(command: string, args?: Record<string, unknown>): unknown {
  switch (command) {
    case "get_firmware_version":
      return "4.0.1";
    case "get_transport_state":
    case "connect_transport":
    case "switch_transport":
      return {
        transport: (args as { transport?: string } | undefined)?.transport ?? "wifi",
        state: "connected",
        address: (args as { address?: string } | undefined)?.address ?? "rsh.local",
        error: null,
      };
    case "begin_pairing":
      return { secondsLeft: 60, password: "abcdefghijklmnopqrstuvwx" };
    case "validate_firmware_image":
      return validateResult;
    case "upload_firmware":
      return undefined;
    default:
      throw new Error(`unknown command "${command}"`);
  }
}

describe("Setup wizard (E3 / UI plan P5)", () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    mockInvoke.mockImplementation(defaultInvoke);
    validateResult = { valid: true, version: "4.0.2", signatureValid: true, errors: [] };
    window.localStorage.clear();
    i18n.addResourceBundle("en", "translation", enStrings, true, true);
    i18n.addResourceBundle("ur", "translation", urStrings, true, true);
    i18n.addResourceBundle("hi", "translation", hiStrings, true, true);
    installTauriMock();
  });

  afterEach(() => {
    removeTauriMock();
    window.localStorage.clear();
  });

  it("exports the screen-module shape the glob discovery needs", () => {
    expect(path).toBe("setup");
    expect(label).toBe("Setup");
    expect(component).toBe(SetupScreen);
    expect(
      toScreenDefinition("src/screens/Setup/index.tsx", {
        path,
        label,
        icon,
        component,
      }),
    ).not.toBeNull();
  });

  it("renders the seven wizard steps in order", () => {
    render(<SetupScreen />);
    const items = Array.from(document.querySelectorAll("ol li")).map((li) =>
      (li.textContent ?? "").replace(/^\d+/, ""),
    );
    expect(items).toEqual([
      "Connect",
      "Pairing",
      "Change password",
      "Pins",
      "Calibrate",
      "Firmware",
      "Finish",
    ]);
    expect(screen.getByText("Step 1 of 7")).toBeInTheDocument();
  });

  it("transport switch calls switch_transport with the chosen transport", async () => {
    render(<SetupScreen />);
    fireEvent.click(screen.getByRole("button", { name: "USB-serial" }));
    fireEvent.change(screen.getByLabelText("Board address"), {
      target: { value: "/dev/ttyUSB0" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Switch transport" }));
    await screen.findByText(/Status: Connected/);
    expect(mockInvoke).toHaveBeenCalledWith(
      "switch_transport",
      { transport: "usb", address: "/dev/ttyUSB0" },
      undefined,
    );
  });

  it("connect calls connect_transport and shows the returned state", async () => {
    render(<SetupScreen />);
    const panel = screen.getByRole("region", { name: "Connect" });
    fireEvent.click(within(panel).getByRole("button", { name: "Connect" }));
    await within(panel).findByText(/Status: Connected/);
    expect(mockInvoke).toHaveBeenCalledWith(
      "connect_transport",
      { transport: "wifi", address: "rsh.local" },
      undefined,
    );
  });

  it("USB connect works with an empty address (the port is auto-detected)", async () => {
    render(<SetupScreen />);
    const panel = screen.getByRole("region", { name: "Connect" });
    fireEvent.click(within(panel).getByRole("button", { name: "USB-serial" }));
    fireEvent.click(within(panel).getByRole("button", { name: "Connect" }));
    await within(panel).findByText(/Status: Connected/);
    expect(mockInvoke).toHaveBeenCalledWith(
      "connect_transport",
      { transport: "usb", address: "" },
      undefined,
    );
  });

  it("a NOT_IMPLEMENTED connect shows the honest banner, not fake state", async () => {
    mockInvoke.mockImplementation((command: string) => {
      if (command === "connect_transport") {
        throw new Error('unknown command "connect_transport"');
      }
      return defaultInvoke(command);
    });
    render(<SetupScreen />);
    const panel = screen.getByRole("region", { name: "Connect" });
    fireEvent.click(within(panel).getByRole("button", { name: "Connect" }));
    await within(panel).findByText(/Not available yet/);
    expect(within(panel).getByText(/Status: Disconnected/)).toBeInTheDocument();
  });

  it("advancing marks the step complete, checkmarks it, and persists progress", async () => {
    const { unmount } = render(<SetupScreen />);
    // H5: Next is gated on the step's success — connect first.
    const panel = screen.getByRole("region", { name: "Connect" });
    fireEvent.click(within(panel).getByRole("button", { name: "Connect" }));
    await within(panel).findByText(/Status: Connected/);
    // The step reports success in a passive effect after the async
    // connect resolves; flush it before clicking Next, or the wizard
    // still sees the step as incomplete (flaky under full-suite load).
    await act(async () => {});
    fireEvent.click(screen.getByRole("button", { name: "Next" }));
    expect(screen.getByText("Step 2 of 7")).toBeInTheDocument();
    expect(
      window.localStorage.getItem("rsh.setup.completed"),
    ).toContain("connect");
    const firstStep = document.querySelectorAll("ol li")[0];
    expect(within(firstStep as HTMLElement).getByLabelText("completed")).toBeInTheDocument();
    unmount();
    render(<SetupScreen />);
    expect(screen.getByText("Step 2 of 7")).toBeInTheDocument();
  });

  it("Next stays gated until the current step succeeds", async () => {
    render(<SetupScreen />);
    fireEvent.click(screen.getByRole("button", { name: "Next" }));
    expect(screen.getByText("Step 1 of 7")).toBeInTheDocument();
    expect(
      screen.getByText(/Complete this step before continuing/),
    ).toBeInTheDocument();
    expect(
      window.localStorage.getItem("rsh.setup.completed"),
    ).not.toContain("connect");
  });

  it("future steps are locked until reached in order", () => {
    render(<SetupScreen />);
    const firmwareChip = screen.getByRole("button", { name: /Firmware/ });
    expect(firmwareChip).toBeDisabled();
  });
});

describe("PairingStep", () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    mockInvoke.mockImplementation(defaultInvoke);
    window.localStorage.clear();
    i18n.addResourceBundle("en", "translation", enStrings, true, true);
    installTauriMock();
  });

  afterEach(() => {
    removeTauriMock();
  });

  it("groups the one-time password in 4 groups of 6 characters", () => {
    expect(groupPassword("abcdefghijklmnopqrstuvwx")).toBe(
      "abcdef ghijkl mnopqr stuvwx",
    );
  });

  it("shows the password once with the shown-once warning, then hides it for good", async () => {
    render(<PairingStep />);
    fireEvent.click(screen.getByRole("button", { name: "Begin pairing" }));
    await screen.findByText("abcdef ghijkl mnopqr stuvwx");
    expect(screen.getByText(/Shown once only/)).toBeInTheDocument();
    expect(screen.getByText("60 seconds left")).toBeInTheDocument();
    fireEvent.click(
      screen.getByRole("button", { name: /I saved it/ }),
    );
    expect(
      screen.queryByText("abcdef ghijkl mnopqr stuvwx"),
    ).not.toBeInTheDocument();
    expect(screen.getByText(/Password hidden/)).toBeInTheDocument();
    expect(mockInvoke).toHaveBeenCalledWith("begin_pairing", {}, undefined);
  });
});

describe("FirmwareStep upload gate (SEC-R2-01)", () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    mockInvoke.mockImplementation(defaultInvoke);
    validateResult = { valid: true, version: "4.0.2", signatureValid: true, errors: [] };
    window.localStorage.clear();
    i18n.addResourceBundle("en", "translation", enStrings, true, true);
    installTauriMock();
  });

  afterEach(() => {
    removeTauriMock();
  });

  function typePath(value: string) {
    fireEvent.change(screen.getByLabelText("Firmware file (.bin)"), {
      target: { value },
    });
  }

  async function validate() {
    fireEvent.click(screen.getByRole("button", { name: "Validate image" }));
  }

  it("keeps Upload disabled until validation passes", () => {
    render(<FirmwareStep />);
    expect(
      screen.getByRole("button", { name: "Upload firmware" }),
    ).toBeDisabled();
  });

  it("invalid image shows the error and Upload stays disabled", async () => {
    validateResult = {
      valid: false,
      version: null,
      signatureValid: false,
      errors: ["signature verification failed"],
    };
    render(<FirmwareStep />);
    typePath("/tmp/firmware-signed.bin");
    await validate();
    await screen.findByText("signature verification failed");
    expect(screen.getByText(/failed validation/)).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Upload firmware" }),
    ).toBeDisabled();
    expect(mockInvoke).not.toHaveBeenCalledWith(
      "upload_firmware",
      expect.anything(),
      expect.anything(),
    );
  });

  it("valid newer image enables Upload and uploading calls upload_firmware", async () => {
    render(<FirmwareStep />);
    typePath("/tmp/firmware-signed.bin");
    await validate();
    const upload = await screen.findByRole("button", {
      name: "Upload firmware",
    });
    await screen.findByText("Image valid — ready to upload.");
    expect(upload).toBeEnabled();
    fireEvent.click(upload);
    await screen.findByText(/Firmware uploaded/);
    expect(mockInvoke).toHaveBeenCalledWith(
      "upload_firmware",
      { path: "/tmp/firmware-signed.bin" },
      undefined,
    );
  });

  it("a downgrade blocks Upload with a clear message", async () => {
    validateResult = {
      valid: true,
      version: "4.0.0",
      signatureValid: true,
      errors: [],
    };
    render(<FirmwareStep />);
    typePath("/tmp/firmware-signed.bin");
    await validate();
    await screen.findByText(/older than the installed 4.0.1/);
    expect(
      screen.getByRole("button", { name: "Upload firmware" }),
    ).toBeDisabled();
  });

  it("parseVersion/compareVersions follow strict a.b.c ordering", () => {
    expect(parseVersion("4.0.1")).toEqual([4, 0, 1]);
    expect(parseVersion("4..1")).toBeNull();
    expect(parseVersion("4.0")).toBeNull();
    expect(parseVersion("4.0.256")).toBeNull();
    expect(compareVersions([4, 0, 0], [4, 0, 1])).toBe(-1);
    expect(compareVersions([4, 0, 1], [4, 0, 1])).toBe(0);
    expect(compareVersions([4, 0, 2], [4, 0, 1])).toBe(1);
  });
});
