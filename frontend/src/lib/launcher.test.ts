import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import {
  canLaunch,
  getLaunchableGames,
  getRunningGame,
  launchGame,
  type GameInfo,
} from "./launcher";

// Same seam as tauri.test.ts: @tauri-apps/api/core's invoke() reads
// window.__TAURI_INTERNALS__.invoke at call time.
const mockInvoke = vi.fn();

function installTauriMock() {
  (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {
    invoke: mockInvoke,
  };
}

function removeTauriMock() {
  delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
}

const ac: GameInfo = {
  id: "assetto-corsa",
  name: "Assetto Corsa",
  boundProfileId: "acc-default",
  running: false,
  installed: true,
};

const acc: GameInfo = {
  id: "assetto-corsa-competizione",
  name: "Assetto Corsa Competizione",
  boundProfileId: null,
  running: true,
  installed: true,
};

const iracing: GameInfo = {
  id: "iracing",
  name: "iRacing",
  boundProfileId: null,
  running: false,
  installed: false,
};

describe("launcher.ts", () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    installTauriMock();
  });

  afterEach(() => {
    removeTauriMock();
  });

  it("launchGame() invokes launch_game with the game id", async () => {
    mockInvoke.mockResolvedValue({ ...ac, running: true });
    await expect(launchGame({ id: "assetto-corsa" })).resolves.toEqual({
      ...ac,
      running: true,
    });
    expect(mockInvoke).toHaveBeenCalledWith(
      "launch_game",
      { id: "assetto-corsa" },
      undefined,
    );
  });

  it("getLaunchableGames() returns only installed, non-running games", () => {
    expect(getLaunchableGames([ac, acc, iracing])).toEqual([ac]);
    expect(getLaunchableGames([])).toEqual([]);
  });

  it("getRunningGame() returns the running game or null", () => {
    expect(getRunningGame([ac, acc, iracing])).toEqual(acc);
    expect(getRunningGame([ac, iracing])).toBeNull();
  });

  it("canLaunch() is true only for installed, non-running games", () => {
    expect(canLaunch(ac)).toBe(true);
    expect(canLaunch(acc)).toBe(false);
    expect(canLaunch(iracing)).toBe(false);
  });
});
