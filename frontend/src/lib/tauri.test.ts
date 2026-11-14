import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import {
  TauriCommandError,
  getConfig,
  getVersion,
  ping,
  IMPLEMENTED_COMMANDS,
  NOT_YET_IMPLEMENTED_COMMANDS,
  subscribeEngineSnapshot,
} from "./tauri";

// The mock lives at the window.__TAURI__ boundary: @tauri-apps/api/core's
// invoke() calls window.__TAURI_INTERNALS__.invoke at call time, so this
// is the exact seam the production code uses. These are test doubles only
// — no fake data is shipped in src/lib/tauri.ts itself.
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

describe("tauri.ts wrappers", () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    mockListen.mockReset();
    installTauriMock();
  });

  afterEach(() => {
    removeTauriMock();
  });

  it("ping() returns pong from the backend", async () => {
    mockInvoke.mockResolvedValue("pong");
    await expect(ping()).resolves.toBe("pong");
    expect(mockInvoke).toHaveBeenCalledWith("ping", {}, undefined);
  });

  it("getVersion() returns the backend version", async () => {
    mockInvoke.mockResolvedValue("1.0.0");
    await expect(getVersion()).resolves.toBe("1.0.0");
  });

  it("a not-yet-implemented command fails loudly as NOT_IMPLEMENTED", async () => {
    mockInvoke.mockRejectedValue(
      new Error('unknown command "get_config"'),
    );
    const error = await getConfig().catch((e: unknown) => e);
    expect(error).toBeInstanceOf(TauriCommandError);
    const typed = error as TauriCommandError;
    expect(typed.code).toBe("NOT_IMPLEMENTED");
    expect(typed.command).toBe("get_config");
    expect(typed.notImplemented).toBe(true);
  });

  it("a backend failure is a typed BACKEND_ERROR, never fake data", async () => {
    mockInvoke.mockRejectedValue(new Error("transport timeout"));
    const error = await getVersion().catch((e: unknown) => e);
    expect(error).toBeInstanceOf(TauriCommandError);
    expect((error as TauriCommandError).code).toBe("BACKEND_ERROR");
  });

  it("calling outside Tauri fails loudly as NOT_IN_TAURI", async () => {
    removeTauriMock();
    const error = await ping().catch((e: unknown) => e);
    expect(error).toBeInstanceOf(TauriCommandError);
    expect((error as TauriCommandError).code).toBe("NOT_IN_TAURI");
  });

  it("only the Phase H5-control + deep-scan Round 2 command set is implemented in src-tauri today", () => {
    expect([...IMPLEMENTED_COMMANDS]).toEqual([
      "ping",
      "get_version",
      "validate_pin_map",
      "validate_firmware_image",
      "list_backups",
      "get_firewall_state",
      "get_autostart",
      "set_autostart",
      "check_for_update",
      "export_diagnostics",
      "get_engine_stats",
      "get_snapshot",
      "get_output_state",
      "set_output_state",
      "set_output_backend",
      "list_output_backends",
      "list_profiles",
      "get_active_profile",
      "activate_profile",
      "create_profile",
      "duplicate_profile",
      "delete_profile",
      "import_profile",
      "export_profile",
      "get_settings",
      "set_settings",
      "enable_auto_switch",
      "led_test",
      "key_reset",
      "factory_reset",
      "confirm_pairing_replacement",
      "import_csharp_profiles",
      "import_csharp_settings",
      "backup_now",
      "restore_backup",
      "list_games",
      "launch_game",
      "stop_game",
      "get_game_state",
      "get_tuning",
      "set_tuning",
      "get_calibration",
      "save_calibration",
      "get_transport_state",
      "connect_transport",
      "disconnect_transport",
      "switch_transport",
      "open_config_folder",
      "get_startup_notice",
      "begin_pairing",
      "get_pairing_status",
      "get_firmware_version",
      "set_password",
      "get_config",
      "stage_config",
      "commit_config",
      "confirm_config",
      "rollback_config",
      "get_config_state",
      "get_pin_map",
      "set_pin_map",
      "upload_firmware",
      "restart_wheel",
    ]);
    // Nothing is missing anymore: the NOT_YET list is empty, and the
    // 11 H5-control commands are implemented (the loop above would
    // catch any command left in NOT_YET that is also implemented).
    expect(NOT_YET_IMPLEMENTED_COMMANDS).toEqual([]);
    expect(IMPLEMENTED_COMMANDS).toContain("get_config");
    expect(IMPLEMENTED_COMMANDS).toContain("upload_firmware");
    expect(IMPLEMENTED_COMMANDS).toContain("set_password");
    expect(IMPLEMENTED_COMMANDS).toContain("restart_wheel");
    expect(NOT_YET_IMPLEMENTED_COMMANDS).not.toContain("begin_pairing");
    expect(NOT_YET_IMPLEMENTED_COMMANDS).not.toContain("get_pairing_status");
    expect(NOT_YET_IMPLEMENTED_COMMANDS).not.toContain("get_firmware_version");
    expect(NOT_YET_IMPLEMENTED_COMMANDS).not.toContain("connect_transport");
    expect(NOT_YET_IMPLEMENTED_COMMANDS).not.toContain("get_transport_state");
    expect(NOT_YET_IMPLEMENTED_COMMANDS).not.toContain("open_config_folder");
    expect(NOT_YET_IMPLEMENTED_COMMANDS).not.toContain("get_startup_notice");
    expect(NOT_YET_IMPLEMENTED_COMMANDS).not.toContain("validate_pin_map");
    expect(NOT_YET_IMPLEMENTED_COMMANDS).not.toContain("validate_firmware_image");
    expect(NOT_YET_IMPLEMENTED_COMMANDS).not.toContain("list_profiles");
    expect(NOT_YET_IMPLEMENTED_COMMANDS).not.toContain("launch_game");
    expect(NOT_YET_IMPLEMENTED_COMMANDS).not.toContain("set_output_state");
    expect(NOT_YET_IMPLEMENTED_COMMANDS).not.toContain("get_engine_stats");
    expect(NOT_YET_IMPLEMENTED_COMMANDS).not.toContain("get_snapshot");
    expect(NOT_YET_IMPLEMENTED_COMMANDS).not.toContain("list_backups");
    expect(NOT_YET_IMPLEMENTED_COMMANDS).not.toContain("get_firewall_state");
  });

  it("subscribeEngineSnapshot listens on rsh://snapshot and forwards payloads", async () => {
    mockListen.mockResolvedValue(() => {});
    const received: unknown[] = [];
    await subscribeEngineSnapshot((snapshot) => {
      received.push(snapshot);
    });
    expect(mockListen).toHaveBeenCalledWith(
      "rsh://snapshot",
      expect.any(Function),
    );
    const handler = mockListen.mock.calls[0][1];
    const snapshot = { seq: 1, steeringDeg: 0 };
    handler({ event: "rsh://snapshot", id: 0, payload: snapshot });
    expect(received).toEqual([snapshot]);
  });
});
