import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import i18n from "@/lib/i18n";
import enStrings from "./strings.en.json";
import { MonitorScreen } from "./index";
import type { EngineSnapshot } from "@/lib/tauri";

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

function fireEventPayload(eventName: string, payload: unknown) {
  handlers[eventName]?.({ event: eventName, id: 0, payload });
}

function fireSnapshot(snapshot: EngineSnapshot) {
  fireEventPayload("rsh://snapshot", snapshot);
}

const sleep = (ms: number) =>
  new Promise((resolve) => setTimeout(resolve, ms));

describe("MonitorScreen", () => {
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
    // Monitor needs no commands; anything invoked is a test failure signal.
    mockInvoke.mockImplementation((cmd: string) =>
      Promise.reject(new Error(`unknown command "${cmd}"`)),
    );
  });

  afterEach(() => {
    removeTauriMock();
  });

  it("packet stats are honestly unavailable before the first snapshot", () => {
    render(<MonitorScreen />);
    expect(
      screen.getByText("Packet stats are not available yet."),
    ).toBeInTheDocument();
    expect(screen.queryByText("200 Hz")).not.toBeInTheDocument();
  });

  it("packet stats update from mocked snapshot events", async () => {
    render(<MonitorScreen />);
    fireSnapshot(makeSnapshot());
    await expect(screen.findByText("200 Hz")).resolves.toBeInTheDocument();
    expect(screen.getByText("0.5 %")).toBeInTheDocument();
    expect(screen.getByText("3.4 ms")).toBeInTheDocument();
    // C3: the board-reported counters render, never a locally counted gap.
    const seqCell = screen.getByText("Board sequence gaps").parentElement;
    expect(seqCell?.textContent).toContain("0");
    const txCell = screen.getByText("TX failures").parentElement;
    expect(txCell?.textContent).toContain("0");
  });

  it("shows the board-reported seqGap/txFail from the snapshot (C3)", async () => {
    render(<MonitorScreen />);
    fireSnapshot(makeSnapshot({ seqGap: 7, txFail: 3 }));
    await screen.findByText("200 Hz");
    await waitFor(() => {
      const seqCell = screen.getByText("Board sequence gaps").parentElement;
      expect(seqCell?.textContent).toContain("7");
      const txCell = screen.getByText("TX failures").parentElement;
      expect(txCell?.textContent).toContain("3");
    });
  });

  it("a seq jump without a board counter change is NOT counted as loss (C3)", async () => {
    render(<MonitorScreen />);
    fireSnapshot(makeSnapshot({ seq: 1, seqGap: 0, txFail: 0 }));
    await screen.findByText("200 Hz");
    await sleep(150);
    fireSnapshot(makeSnapshot({ seq: 4, seqGap: 0, txFail: 0 }));
    await waitFor(() => {
      const seqCell = screen.getByText("Board sequence gaps").parentElement;
      expect(seqCell?.textContent).toContain("0");
    });
  });

  it("an unmeasured end-to-end latency renders as not measured, never 0", async () => {
    render(<MonitorScreen />);
    fireSnapshot(makeSnapshot({ stats: { ...engineStats, e2eMs: 0 } }));
    await waitFor(() => {
      expect(screen.getByText("Not measured")).toBeInTheDocument();
    });
    expect(screen.queryByText("0 ms")).not.toBeInTheDocument();
  });

  it("charts update from mocked snapshot events", async () => {
    render(<MonitorScreen />);
    fireSnapshot(
      makeSnapshot({
        steeringDeg: 45,
        pedals: { accel: 0.75, brake: 0.1, clutch: null },
      }),
    );
    await expect(screen.findByText("45°")).resolves.toBeInTheDocument();
    expect(screen.getByText(/75 %/)).toBeInTheDocument();
    expect(screen.getByText(/10 %/)).toBeInTheDocument();
  });

  it("button grid updates from button-press events", async () => {
    render(<MonitorScreen />);
    expect(
      screen.getByText(/No button activity yet/),
    ).toBeInTheDocument();
    fireEventPayload("rsh://button-press", { button: 5, pressed: true });
    await expect(screen.findByText("Pressed")).resolves.toBeInTheDocument();
    expect(screen.getByText("5")).toBeInTheDocument();
    fireEventPayload("rsh://button-press", { button: 5, pressed: false });
    await waitFor(() => {
      expect(screen.getByText("Released")).toBeInTheDocument();
    });
    expect(screen.queryByText("Pressed")).not.toBeInTheDocument();
  });

  it("encoders show the honest not-available state", () => {
    render(<MonitorScreen />);
    expect(
      screen.getByText(/Encoder data is not available yet/),
    ).toBeInTheDocument();
  });

  it("encoders render from the snapshot when present (M12)", async () => {
    render(<MonitorScreen />);
    fireSnapshot(
      makeSnapshot({
        encoders: [
          {
            index: 0,
            count: 12,
            delta: 2,
            cwButton: 65,
            ccwButton: 66,
            cwPressed: true,
            ccwPressed: false,
          },
        ],
        encoderButtons: [65],
      }),
    );
    await expect(screen.findByText(/Encoder 1/)).resolves.toBeInTheDocument();
    expect(screen.getByText(/12/)).toBeInTheDocument();
  });

  it("event log records connection changes from snapshots", async () => {
    render(<MonitorScreen />);
    fireSnapshot(makeSnapshot({ seq: 1, connection: "connected" }));
    await screen.findByText("200 Hz");
    await sleep(150);
    fireSnapshot(makeSnapshot({ seq: 2, connection: "disconnected" }));
    await expect(
      screen.findByText("Connection changed to disconnected"),
    ).resolves.toBeInTheDocument();
  });

  it("event log records game state changes", async () => {
    render(<MonitorScreen />);
    fireEventPayload("rsh://game-state", {
      id: "assetto-corsa",
      name: "Assetto Corsa",
      boundProfileId: "p1",
      running: true,
      installed: true,
    });
    await expect(
      screen.findByText("Assetto Corsa started"),
    ).resolves.toBeInTheDocument();
    fireEventPayload("rsh://game-state", {
      id: "assetto-corsa",
      name: "Assetto Corsa",
      boundProfileId: "p1",
      running: false,
      installed: true,
    });
    await expect(
      screen.findByText("Assetto Corsa stopped"),
    ).resolves.toBeInTheDocument();
  });

  it("event log records a config rollback", async () => {
    render(<MonitorScreen />);
    fireEventPayload("rsh://config-rollback", {
      reason: "confirm window expired",
    });
    await expect(
      screen.findByText(
        "Configuration rolled back: confirm window expired",
      ),
    ).resolves.toBeInTheDocument();
  });

  it("subscribes to the snapshot and button-press events and invokes no commands", () => {
    render(<MonitorScreen />);
    expect(Object.keys(handlers)).toContain("rsh://snapshot");
    expect(Object.keys(handlers)).toContain("rsh://button-press");
    expect(mockInvoke).not.toHaveBeenCalled();
  });
});
