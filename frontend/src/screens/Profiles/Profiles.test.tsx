import { describe, it, expect, vi, beforeEach, beforeAll, afterEach } from "vitest";
import { render, screen, fireEvent, waitFor, within } from "@testing-library/react";
import i18n from "@/lib/i18n";
import { ProfilesScreen, isValidProfileName } from "./index";
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

function flattenKeys(value: unknown, prefix = ""): string[] {
  if (value === null || typeof value !== "object") return [prefix];
  return Object.entries(value as Record<string, unknown>).flatMap(
    ([key, child]) => flattenKeys(child, prefix ? `${prefix}.${key}` : key),
  );
}

const PROFILES = [
  { id: "p1", name: "GT3", game: "Assetto Corsa", active: true },
  { id: "p2", name: "Drift", game: null, active: false },
];

describe("Profiles screen", () => {
  beforeAll(() => {
    i18n.addResourceBundle("en", "translation", en, true, true);
    i18n.addResourceBundle("ur", "translation", ur, true, true);
    i18n.addResourceBundle("hi", "translation", hi, true, true);
  });

  beforeEach(() => {
    mockInvoke.mockReset();
    mockInvoke.mockImplementation((cmd: string) => {
      switch (cmd) {
        case "list_profiles":
          return Promise.resolve([...PROFILES]);
        case "get_active_profile":
          return Promise.resolve(PROFILES[0]);
        case "activate_profile":
          return Promise.resolve({ ...PROFILES[1], active: true });
        case "create_profile":
          return Promise.resolve({
            id: "p3",
            name: "New",
            game: null,
            active: false,
          });
        case "duplicate_profile":
          return Promise.resolve({
            id: "p4",
            name: "Drift (copy)",
            game: null,
            active: false,
          });
        case "delete_profile":
          return Promise.resolve(undefined);
        default:
          return Promise.reject(new Error(`unknown command "${cmd}"`));
      }
    });
    installTauriMock();
  });

  afterEach(() => {
    removeTauriMock();
  });

  it("lists profiles with names, bound game and the active profile", async () => {
    render(<ProfilesScreen />);
    expect(await screen.findByText("GT3")).toBeInTheDocument();
    expect(screen.getByText("Drift")).toBeInTheDocument();
    expect(screen.getByText("Assetto Corsa")).toBeInTheDocument();
    expect(screen.getByText("No game bound")).toBeInTheDocument();
    expect(screen.getByText("Active")).toBeInTheDocument();
    // Backup indicator: current is live; older generations not reported.
    expect(screen.getByText("Backup generations")).toBeInTheDocument();
    expect(screen.getByText("Live")).toBeInTheDocument();
    expect(screen.getAllByText("Not reported yet")).toHaveLength(2);
  });

  it("create calls create_profile with the entered name", async () => {
    render(<ProfilesScreen />);
    await screen.findByText("GT3");
    fireEvent.change(screen.getByLabelText("Profile name"), {
      target: { value: "New" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Create profile" }));
    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith(
        "create_profile",
        { name: "New" },
        undefined,
      );
    });
  });

  it("create with an empty name shows an error and calls nothing", async () => {
    render(<ProfilesScreen />);
    await screen.findByText("GT3");
    fireEvent.click(screen.getByRole("button", { name: "Create profile" }));
    expect(
      await screen.findByText("Enter a profile name."),
    ).toBeInTheDocument();
    expect(mockInvoke).not.toHaveBeenCalledWith(
      "create_profile",
      expect.anything(),
      undefined,
    );
  });

  it("activate calls activate_profile for the chosen profile", async () => {
    render(<ProfilesScreen />);
    await screen.findByText("Drift");
    const rows = screen.getAllByRole("button", { name: "Activate" });
    // GT3 is already active (its button is disabled); Drift is second.
    expect(rows).toHaveLength(2);
    fireEvent.click(rows[1]);
    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith(
        "activate_profile",
        { id: "p2" },
        undefined,
      );
    });
  });

  it("duplicate calls duplicate_profile for the chosen profile", async () => {
    render(<ProfilesScreen />);
    await screen.findByText("Drift");
    const rows = screen.getAllByRole("button", { name: "Duplicate" });
    fireEvent.click(rows[1]);
    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith(
        "duplicate_profile",
        { id: "p2" },
        undefined,
      );
    });
  });

  it("delete asks for confirmation, then calls delete_profile", async () => {
    render(<ProfilesScreen />);
    await screen.findByText("Drift");
    const rows = screen.getAllByRole("button", { name: "Delete" });
    fireEvent.click(rows[1]);
    const dialog = await screen.findByRole("dialog");
    expect(
      within(dialog).getByText("Delete profile Drift? This cannot be undone."),
    ).toBeInTheDocument();
    fireEvent.click(within(dialog).getByRole("button", { name: "Delete" }));
    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith(
        "delete_profile",
        { id: "p2" },
        undefined,
      );
    });
  });

  it("shows an honest not-available state when list_profiles is not implemented", async () => {
    mockInvoke.mockImplementation((cmd: string) =>
      Promise.reject(new Error(`unknown command "${cmd}"`)),
    );
    render(<ProfilesScreen />);
    expect(
      await screen.findByText(/not available yet/i),
    ).toBeInTheDocument();
  });

  it("strings have key parity across en/ur/hi", () => {
    expect(flattenKeys(en).sort()).toEqual(flattenKeys(ur).sort());
    expect(flattenKeys(en).sort()).toEqual(flattenKeys(hi).sort());
  });
});

describe("isValidProfileName", () => {
  it("accepts normal names and rejects empty / over-long names", () => {
    expect(isValidProfileName("GT3")).toBe(true);
    expect(isValidProfileName("  ")).toBe(false);
    expect(isValidProfileName("")).toBe(false);
    expect(isValidProfileName("x".repeat(64))).toBe(true);
    expect(isValidProfileName("x".repeat(65))).toBe(false);
  });
});
