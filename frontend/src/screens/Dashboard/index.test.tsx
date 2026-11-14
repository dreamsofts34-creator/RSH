import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, waitFor, fireEvent } from "@testing-library/react";
import i18n from "@/lib/i18n";
import enStrings from "./strings.en.json";
import { DashboardScreen } from "./index";
import type { EngineSnapshot, GameInfo } from "@/lib/tauri";

// Mocks live at the window.__TAURI__ boundary only: invoke + the event
// stream. No production code is mocked.
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(),
}));

import { listen } from "@tauri-apps/api/event";

const mockListen = vi.mocked(listen);
const mockInvoke = vi.fn();

type EventHandler = (event: {
  event: string;
  id: number;
  payload: unknown;
}) => void;

const handlers: Record<string, EventHandler> = {};

function installTauriMock() {
  (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {
    invoke: mockInvoke,
  };
}

function removeTauriMock() {
  delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
}

const transportState = {
  transport: "wifi",
  state: "connected",
  address: "192.168.4.1",
  error: null,
};

const games: GameInfo[] = [
  {
    id: "assetto-corsa",
    name: "Assetto Corsa",
    boundProfileId: "p1",
    running: false,
    installed: true,
  },
  {
    id: "assetto-corsa-competizione",
    name: "Assetto Corsa Competizione",
    boundProfileId: null,
    running: false,
    installed: true,
  },
  {
    id: "iracing",
    name: "iRacing",
    boundProfileId: "p2",
    running: false,
    installed: false,
  },
  {
    id: "dirt-rally-2",
    name: "Dirt Rally 2.0",
    boundProfileId: null,
    running: false,
    installed: true,
  },
];

const profiles = [
  { id: "p1", name: "GT3 Setup", game: "assetto-corsa", active: true },
  { id: "p2", name: "Oval", game: "iracing", active: false },
];

const engineStats = {
  loopHz: 200,
  lossPct: 0.5,
  jitterMsP99: 3.4,
  outputLatencyMs: 1.2,
  e2eMs: 4.8,
};

function makeSnapshot(overrides: Partial<EngineSnapshot> = {}): EngineSnapshot {
  return {
    steeringDeg: 0,
    pedals: { accel: 0, brake: 0, clutch: null },
    connection: "connected",
    stats: engineStats,
    activeProfileId: "p1",
    seq: 1,
    seqGap: 0,
    txFail: 0,
    ...overrides,
  };
}

function fireSnapshot(snapshot: EngineSnapshot) {
  handlers["rsh://snapshot"]?.({
    event: "rsh://snapshot",
    id: 0,
    payload: snapshot,
  });
}

function installDefaultInvoke() {
  mockInvoke.mockImplementation((cmd: string, args?: Record<string, unknown>) => {
    switch (cmd) {
      case "get_transport_state":
        return Promise.resolve(transportState);
      case "list_games":
        return Promise.resolve(games);
      case "list_profiles":
        return Promise.resolve(profiles);
      case "get_active_profile":
        return Promise.resolve(profiles[0]);
      case "get_engine_stats":
        return Promise.resolve(engineStats);
      case "launch_game": {
        const game = games.find((g) => g.id === args?.["id"]);
        return Promise.resolve({ ...game, running: true });
      }
      case "stop_game": {
        const game = games.find((g) => g.id === args?.["id"]);
        return Promise.resolve({ ...game, running: false });
      }
      case "restart_wheel":
        return Promise.resolve(undefined);
      default:
        return Promise.reject(new Error(`unknown command "${cmd}"`));
    }
  });
}

describe("DashboardScreen", () => {
  beforeEach(() => {
    i18n.addResourceBundle("en", "translation", enStrings, true, true);
    mockInvoke.mockReset();
    mockListen.mockReset();
    for (const key of Object.keys(handlers)) delete handlers[key];
    mockListen.mockImplementation(((event: string, handler: EventHandler) => {
      handlers[event] = handler;
      return Promise.resolve(() => {});
    }) as unknown as typeof listen);
    installTauriMock();
    installDefaultInvoke();
  });

  afterEach(() => {
    removeTauriMock();
  });

  it("shows the connection card from get_transport_state", async () => {
    render(<DashboardScreen />);
    await expect(screen.findByText("192.168.4.1")).resolves.toBeInTheDocument();
    expect(screen.getByText("Wi-Fi")).toBeInTheDocument();
    expect(screen.getByText("Connected")).toBeInTheDocument();
  });

  it("launcher calls launch_game with the right game id", async () => {
    render(<DashboardScreen />);
    await screen.findByText("Assetto Corsa");
    const launchButtons = screen.getAllByRole("button", { name: "Launch" });
    fireEvent.click(launchButtons[0]);
    await waitFor(() => {
      expect(
        mockInvoke.mock.calls.some(
          ([cmd, args]) =>
            cmd === "launch_game" &&
            (args as { id?: string } | undefined)?.id === "assetto-corsa",
        ),
      ).toBe(true);
    });
  });

  it("an unconfigured game renders the honest state", async () => {
    render(<DashboardScreen />);
    await screen.findByText("Assetto Corsa Competizione");
    // Two games in the mock have no bound profile (ACC, Dirt Rally 2.0).
    expect(screen.getAllByText("Not configured")).toHaveLength(2);
    expect(screen.getAllByText(/No profile bound/)).toHaveLength(2);
  });

  it("a game with a bound profile shows the profile name", async () => {
    render(<DashboardScreen />);
    await screen.findByText("Assetto Corsa");
    const row = screen.getByText("Assetto Corsa").closest("li");
    expect(row?.textContent).toContain("GT3 Setup");
  });

  it("a not-installed game cannot be launched", async () => {
    render(<DashboardScreen />);
    await screen.findByText("iRacing");
    expect(screen.getByText("Not installed")).toBeInTheDocument();
    const launchButtons = screen.getAllByRole("button", { name: "Launch" });
    // iRacing is the third game row; its Launch button is disabled.
    expect(launchButtons[2]).toBeDisabled();
  });

  it("a running game blocks launching the others", async () => {
    render(<DashboardScreen />);
    await screen.findByText("Dirt Rally 2.0");
    const launchButtons = screen.getAllByRole("button", { name: "Launch" });
    fireEvent.click(launchButtons[3]);
    await waitFor(() => {
      // One note per non-running game row.
      expect(
        screen.getAllByText("Stop the running game first."),
      ).toHaveLength(3);
    });
    expect(screen.getAllByRole("button", { name: "Launch" })[0]).toBeDisabled();
    expect(screen.getByRole("button", { name: "Stop" })).toBeInTheDocument();
  });

  it("stats render only backend-reported numbers", async () => {
    render(<DashboardScreen />);
    await screen.findByText("192.168.4.1");
    fireSnapshot(makeSnapshot());
    await expect(screen.findByText("200 Hz")).resolves.toBeInTheDocument();
    expect(screen.getByText("0.5 %")).toBeInTheDocument();
    expect(screen.getByText("3.4 ms")).toBeInTheDocument();
  });

  it("an unmeasured end-to-end latency renders as not measured, never 0", async () => {
    render(<DashboardScreen />);
    await screen.findByText("192.168.4.1");
    fireSnapshot(
      makeSnapshot({ stats: { ...engineStats, e2eMs: 0 } }),
    );
    await waitFor(() => {
      expect(screen.getByText("Not measured")).toBeInTheDocument();
    });
    expect(screen.queryByText("0 ms")).not.toBeInTheDocument();
  });

  it("wheel and pedals update from mocked snapshot events", async () => {
    render(<DashboardScreen />);
    await screen.findByText("192.168.4.1");
    fireSnapshot(
      makeSnapshot({
        steeringDeg: 90,
        pedals: { accel: 0.5, brake: 0.25, clutch: null },
      }),
    );
    await expect(screen.findByText("90°")).resolves.toBeInTheDocument();
    expect(screen.getByText("50 %")).toBeInTheDocument();
    expect(screen.getByText("25 %")).toBeInTheDocument();
    // No clutch channel in this snapshot: no clutch bar is rendered.
    expect(screen.queryByText("Clutch")).not.toBeInTheDocument();
  });

  it("restart wheel calls the restart_wheel wrapper", async () => {
    render(<DashboardScreen />);
    await screen.findByText("192.168.4.1");
    fireEvent.click(screen.getByRole("button", { name: "Restart wheel" }));
    await waitFor(() => {
      expect(
        mockInvoke.mock.calls.some(([cmd]) => cmd === "restart_wheel"),
      ).toBe(true);
    });
    await expect(
      screen.findByText("Restart requested."),
    ).resolves.toBeInTheDocument();
  });

  it("the launcher is honestly unavailable when list_games is NOT_IMPLEMENTED", async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === "list_games") {
        return Promise.reject(new Error('unknown command "list_games"'));
      }
      switch (cmd) {
        case "get_transport_state":
          return Promise.resolve(transportState);
        case "list_profiles":
          return Promise.resolve(profiles);
        case "get_active_profile":
          return Promise.resolve(profiles[0]);
        case "get_engine_stats":
          return Promise.resolve(engineStats);
        default:
          return Promise.reject(new Error(`unknown command "${cmd}"`));
      }
    });
    render(<DashboardScreen />);
    await expect(
      screen.findByText("Game launcher is not available yet."),
    ).resolves.toBeInTheDocument();
  });

  it("stats are honestly unavailable when get_engine_stats is NOT_IMPLEMENTED", async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === "get_engine_stats") {
        return Promise.reject(new Error('unknown command "get_engine_stats"'));
      }
      switch (cmd) {
        case "get_transport_state":
          return Promise.resolve(transportState);
        case "list_games":
          return Promise.resolve(games);
        case "list_profiles":
          return Promise.resolve(profiles);
        case "get_active_profile":
          return Promise.resolve(profiles[0]);
        default:
          return Promise.reject(new Error(`unknown command "${cmd}"`));
      }
    });
    render(<DashboardScreen />);
    await expect(
      screen.findByText("Engine stats are not available yet."),
    ).resolves.toBeInTheDocument();
    expect(screen.queryByText("200 Hz")).not.toBeInTheDocument();
  });
});
