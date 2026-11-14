import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

/**
 * Tauri command surface (UI plan §6 + Software plan contracts).
 *
 * Typed `invoke` wrappers for every command the screens will need.
 * The frontend NEVER invents data: a wrapper either returns what the
 * Rust backend returned or throws a `TauriCommandError`. There is no
 * mock/fake data path in production code — test doubles live only in
 * `*.test.*` files.
 *
 * ------------------------------------------------------------------
 * WHAT EXISTS IN src-tauri TODAY (2026-11-14, Phase H3):
 *   - `ping`            -> "pong"
 *   - `get_version`     -> "1.0.0"
 *   - `validate_pin_map`      -> PinMapValidation (rsh-protocol::validate_blob)
 *   - `validate_firmware_image` -> FirmwareValidation (rsh-security::verify_firmware_image)
 *   - `list_backups`, `get_firewall_state`, `get_autostart`,
 *     `set_autostart`, `check_for_update`, `export_diagnostics` (Phase F)
 *   - `get_engine_stats`, `get_snapshot`, `get_output_state`,
 *     `set_output_state`, `set_output_backend` (Phase G0: AppState)
 *   - `list_profiles`, `get_active_profile`, `activate_profile`,
 *     `create_profile`, `duplicate_profile`, `delete_profile`,
 *     `import_profile`, `export_profile` (Phase G0: AppState)
 *   - `list_games`, `launch_game`, `stop_game`, `get_game_state`
 *     (Phase G0: AppState)
 *   - `get_settings`, `set_settings`, `backup_now`, `restore_backup`,
 *     `get_tuning`, `set_tuning`, `get_calibration`, `save_calibration`
 *     (Phase G0: AppState)
 *   - `get_transport_state`, `connect_transport`,
 *     `disconnect_transport`, `switch_transport` (Phase H3: thin
 *     wrappers over `AppStateCore::transport_state` /
 *     `connect_transport` — the live transport is real; the serial
 *     port is auto-detected and a Wi-Fi IP literal is persisted as
 *     `esp_ip`, while `rsh.local` is never persisted as an IP)
 *   - `open_config_folder` (Phase H3: OS file manager on the data dir)
 *   - `get_startup_notice` (Phase H3: corrupt-store repair /
 *     autostart-repair outcome, shown once)
 *   - `begin_pairing`, `get_pairing_status`, `get_firmware_version`
 *     (Phase H5: thin wrappers over the `AppStateCore` session
 *     activator — pairing status and firmware version read the
 *     activator's records, which exist only while a session is live)
 *   - `set_password`, `get_config`, `stage_config`, `commit_config`,
 *     `confirm_config`, `rollback_config`, `get_config_state`,
 *     `get_pin_map`, `set_pin_map`, `upload_firmware`, `restart_wheel`
 *     (Phase H5-control: thin wrappers over the
 *     run-a-control-command-under-a-fresh-session helper — every
 *     command opens its own fresh session with a fresh nonce and a
 *     signed `GET_CAPS` first, then closes it and zeroizes the key)
 *   - 30 Hz `rsh://snapshot` events + `rsh://button-press` events,
 *     emitted by the AppState snapshot emitter thread.
 * 56 commands in `IMPLEMENTED_COMMANDS` below; the
 * NOT_YET_IMPLEMENTED list is empty — every command the screens need
 * has a live backend now.
 * ------------------------------------------------------------------
 * How a not-yet-implemented command fails: Tauri rejects the invoke with
 * an "unknown command" error, which `invokeCommand` maps to
 * `TauriCommandError` with `code: "NOT_IMPLEMENTED"`. Callers must treat
 * that as "backend feature not built yet", never as data. The
 * `NOT_YET_IMPLEMENTED_COMMANDS` list documents the known-missing set so
 * screens can show an honest "not available yet" state instead of
 * retrying forever.
 * ------------------------------------------------------------------
 */

/** Error thrown by every wrapper when the backend call fails. */
export class TauriCommandError extends Error {
  readonly command: string;
  readonly code: "NOT_IMPLEMENTED" | "BACKEND_ERROR" | "NOT_IN_TAURI";
  /** True when the Rust side has not implemented this command yet. */
  readonly notImplemented: boolean;

  constructor(
    command: string,
    code: TauriCommandError["code"],
    message: string,
  ) {
    super(message);
    this.name = "TauriCommandError";
    this.command = command;
    this.code = code;
    this.notImplemented = code === "NOT_IMPLEMENTED";
  }
}

/**
 * Commands known to be missing from src-tauri today. Kept in sync with
 * `src-tauri/src/main.rs` by hand — when a worker registers a command,
 * remove it from this list. Empty since Phase H5-control: all 11
 * remaining commands (`set_password`, config staging, pin-map get/set,
 * firmware upload, `restart_wheel`) landed on the
 * run-a-control-command-under-a-fresh-session helper, so no command
 * the screens need is missing anymore.
 */
export const NOT_YET_IMPLEMENTED_COMMANDS: readonly string[] = [];

/** Commands that exist in src-tauri today (Phase H5-control: 56). */
export const IMPLEMENTED_COMMANDS: readonly string[] = [
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
  "list_games",
  "launch_game",
  "stop_game",
  "get_game_state",
  "get_settings",
  "set_settings",
  "backup_now",
  "restore_backup",
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
];

function isNotImplementedMessage(message: string): boolean {
  return /unknown command|command .* not found|not implemented/i.test(message);
}

function isNotInTauriMessage(message: string): boolean {
  return /__TAURI_INTERNALS__|not running inside tauri|window\.__TAURI__/i.test(
    message,
  );
}

/**
 * Single choke point for every command. Maps raw invoke failures onto
 * typed errors; never returns a default/fake value.
 */
async function invokeCommand<T>(
  command: string,
  args?: Record<string, unknown>,
): Promise<T> {
  // Fail loudly outside the Tauri runtime instead of letting the
  // @tauri-apps/api internals throw an opaque TypeError.
  if (
    typeof window === "undefined" ||
    (window as unknown as { __TAURI_INTERNALS__?: unknown })
      .__TAURI_INTERNALS__ === undefined
  ) {
    throw new TauriCommandError(
      command,
      "NOT_IN_TAURI",
      `Tauri command "${command}" was called outside the Tauri app (no Tauri runtime).`,
    );
  }
  let result: T;
  try {
    result = await invoke<T>(command, args);
  } catch (error) {
    const message =
      error instanceof Error ? error.message : String(error ?? "unknown error");
    if (isNotImplementedMessage(message)) {
      throw new TauriCommandError(
        command,
        "NOT_IMPLEMENTED",
        `Tauri command "${command}" is not implemented in the Rust backend yet.`,
      );
    }
    if (isNotInTauriMessage(message)) {
      throw new TauriCommandError(
        command,
        "NOT_IN_TAURI",
        `Tauri command "${command}" was called outside the Tauri app (no Tauri runtime).`,
      );
    }
    throw new TauriCommandError(
      command,
      "BACKEND_ERROR",
      `Tauri command "${command}" failed: ${message}`,
    );
  }
  return result;
}

/* ------------------------------------------------------------------ */
/* Shared payload types (mirror the Rust serde shapes; the Rust side   */
/* is the authority — if a field changes there, update it here too).   */
/* ------------------------------------------------------------------ */

export type TransportKind = "wifi" | "usb";
export type ConnectionState =
  | "disconnected"
  | "connecting"
  | "connected"
  | "pairing"
  | "error";

export interface TransportState {
  transport: TransportKind;
  state: ConnectionState;
  /** Board address (IP for Wi-Fi, port path for USB). */
  address: string;
  /** Last error message, if the state is `error`. */
  error: string | null;
  /** H1 SEC-E-14: true only once a session authenticated and the
   * session gate opened — false means stream frames are being held,
   * never silently dropped. */
  sessionActive?: boolean;
  /** H1 SEC-E-14: stream frames held by the closed gate. */
  framesHeldByGate?: number;
}

export interface PairingStatus {
  /** Seconds left in the 60 s pairing window; 0 when no window is open. */
  secondsLeft: number;
  /** The one-time device password, shown once, or null when not pairing. */
  password: string | null;
  /** The last session-activation failure, if any (null when the last
   * attempt succeeded or none ran yet). */
  error?: string | null;
}

export type ConfigState = "clean" | "staged" | "pending_confirm" | "rolled_back";

export interface PinMapValidation {
  valid: boolean;
  errors: string[];
  warnings: string[];
}

export interface PinAssignment {
  /** Module/function name, e.g. "steering", "brake", "led". */
  function: string;
  /** GPIO pin number on the ESP32 (single-pin functions). */
  pin: number;
  /**
   * GPIO pins for multi-pin functions (shifter: 6 gear pins, encoders:
   * 2 pins per encoder, button matrix: rows + cols pins). When present
   * and non-empty it wins over `pin`; absent for single-pin functions.
   */
  pins?: number[];
}

export interface ProfileSummary {
  id: string;
  name: string;
  game: string | null;
  active: boolean;
}

export interface GameInfo {
  id: string;
  name: string;
  boundProfileId: string | null;
  running: boolean;
  installed: boolean;
}

/**
 * Engine stats (Phase F snapshot API, `rsh-app-core/src/snapshot.rs`).
 *
 * Nullable fields are `null` while unmeasured — never a fake 0. Only
 * `lossPct` is always a number: 0 % loss is a real, measured value.
 * `loopHz` is measured by the backend's loop-rate tracker (ticks over a
 * sliding 1 s window); `null` until at least 2 ticks are measured.
 */
export interface EngineStats {
  /** Engine loop rate in Hz (target 200); null while unmeasured. */
  loopHz: number | null;
  /** Packet loss in percent, sliding window. */
  lossPct: number;
  /** Jitter p99 in milliseconds; null while unmeasured. */
  jitterMsP99: number | null;
  /** Output latency in milliseconds; null while unmeasured. */
  outputLatencyMs: number | null;
  /** End-to-end latency in milliseconds (ping-derived); null while unmeasured. */
  e2eMs: number | null;
}

/** One encoder in a snapshot: cumulative count, delta, derived buttons 65–72. */
export interface EncoderSnapshot {
  /** Encoder index, 0-based (displayed 1-based). */
  index: number;
  /** Cumulative i16 count from the latest stream frame. */
  count: number;
  /** Wrap-aware delta since the previous snapshot (0 on the first sample). */
  delta: number;
  /** Clockwise virtual button number (65 + 2*index). */
  cwButton: number;
  /** Counter-clockwise virtual button number (66 + 2*index). */
  ccwButton: number;
  /** True when this encoder stepped clockwise in this snapshot's tick. */
  cwPressed: boolean;
  /** True when this encoder stepped counter-clockwise in this snapshot's tick. */
  ccwPressed: boolean;
}

/** One 30 Hz engine snapshot (UI plan §6.1, event `rsh://snapshot`). */
export interface EngineSnapshot {
  /** Steering angle in degrees, -450..450 (0..900° range). */
  steeringDeg: number;
  /** Pedal positions 0..1: accel, brake, clutch (clutch only if present). */
  pedals: { accel: number; brake: number; clutch: number | null };
  connection: ConnectionState;
  stats: EngineStats;
  activeProfileId: string | null;
  /** Monotonic sequence number; gaps mean dropped snapshots. */
  seq: number;
  /** Encoder snapshots (one per encoder present in the frame). */
  encoders?: EncoderSnapshot[];
  /** Virtual buttons pressed this tick (subset of 65–72). */
  encoderButtons?: number[];
}

export type TuningSettings = {
  module: string;
  deadzone: number;
  curve: number;
  /** Raw min/max in mV; span must stay >= 20% of full scale. */
  minMv: number;
  maxMv: number;
  smoothing: number;
  /** Pedal bite point, 0..1 (clutch / brake only; absent elsewhere). */
  bitePoint?: number;
  /** Shifter gate type (shifter module only). */
  shifterGate?: "sequential" | "h-pattern";
  /** Button number (1–32) the horn is bound to (horn module only). */
  hornButton?: number;
  /** LED strip brightness in percent, 0–100 (LED module only). */
  ledBrightness?: number;
  /** LED strip pattern (LED module only). */
  ledPattern?: "off" | "solid" | "blink" | "chase" | "rainbow";
};

export interface Calibration {
  steeringCenterMv: number;
  pedalMinMv: Record<string, number>;
  pedalMaxMv: Record<string, number>;
}

export interface FirmwareValidation {
  valid: boolean;
  version: string | null;
  /** Ed25519 signature check result. */
  signatureValid: boolean;
  errors: string[];
}

/* ------------------------------------------------------------------ */
/* Implemented today (src-tauri registers these).                      */
/* ------------------------------------------------------------------ */

/** Liveness probe — returns the literal string "pong". */
export function ping(): Promise<string> {
  return invokeCommand<string>("ping");
}

/** Application version, e.g. "1.0.0". */
export function getVersion(): Promise<string> {
  return invokeCommand<string>("get_version");
}

/* ------------------------------------------------------------------ */
/* Transport + pairing — transport implemented in src-tauri (H3);     */
/* pairing implemented in src-tauri (H5, session activator);          */
/* `set_password` implemented in src-tauri (H5-control, fresh        */
/* session — the board restarts its AP after replying).               */
/* ------------------------------------------------------------------ */

export function connectTransport(args: {
  transport: TransportKind;
  address: string;
}): Promise<TransportState> {
  return invokeCommand<TransportState>("connect_transport", args);
}

export function disconnectTransport(): Promise<TransportState> {
  return invokeCommand<TransportState>("disconnect_transport");
}

export function getTransportState(): Promise<TransportState> {
  return invokeCommand<TransportState>("get_transport_state");
}

export function switchTransport(args: {
  transport: TransportKind;
  address: string;
}): Promise<TransportState> {
  return invokeCommand<TransportState>("switch_transport", args);
}

/** Begins pairing: triggers a session-activation attempt and returns
 * the pairing status (poll `getPairingStatus` while the window is
 * open — the `HELLO` arrives asynchronously in the forwarder). */
export function beginPairing(): Promise<PairingStatus> {
  return invokeCommand<PairingStatus>("begin_pairing");
}

export function getPairingStatus(): Promise<PairingStatus> {
  return invokeCommand<PairingStatus>("get_pairing_status");
}

/** Sets a new device password (8..63 printable ASCII) under a fresh
 * session (H5-control). The new password is stored as the Wi-Fi / OTA
 * password too; the board restarts its AP after replying, so the user
 * must rejoin the board's AP with the new password. */
export function setPassword(args: { password: string }): Promise<void> {
  return invokeCommand<void>("set_password", args);
}

/* ------------------------------------------------------------------ */
/* Config staging — stage -> commit -> confirm (30 s) -> rollback,     */
/* implemented in src-tauri (H5-control, one fresh session per step;  */
/* the transaction record lives in the app state between steps). The   */
/* step commands return nothing — the new state comes from            */
/* `getConfigState` (the screens await the step, they do not read a   */
/* return value).                                                      */
/* ------------------------------------------------------------------ */

export function getConfig(): Promise<Record<string, unknown>> {
  return invokeCommand<Record<string, unknown>>("get_config");
}

export function stageConfig(args: {
  config: Record<string, unknown>;
}): Promise<void> {
  return invokeCommand<void>("stage_config", args);
}

export function commitConfig(): Promise<void> {
  return invokeCommand<void>("commit_config");
}

export function confirmConfig(): Promise<void> {
  return invokeCommand<void>("confirm_config");
}

export function rollbackConfig(): Promise<void> {
  return invokeCommand<void>("rollback_config");
}

export function getConfigState(): Promise<ConfigState> {
  return invokeCommand<ConfigState>("get_config_state");
}

/* ------------------------------------------------------------------ */
/* GPIO pin map — implemented in src-tauri (H5-control): `get_pin_map` */
/* reads the single-pin functions the editor can round-trip,          */
/* `set_pin_map` merges the full map into the live blob by role and    */
/* stages + commits it under one fresh session.                        */
/* ------------------------------------------------------------------ */

export function validatePinMap(args: {
  pins: PinAssignment[];
}): Promise<PinMapValidation> {
  return invokeCommand<PinMapValidation>("validate_pin_map", args);
}

export function getPinMap(): Promise<PinAssignment[]> {
  return invokeCommand<PinAssignment[]>("get_pin_map");
}

export function setPinMap(args: {
  pins: PinAssignment[];
}): Promise<PinMapValidation> {
  return invokeCommand<PinMapValidation>("set_pin_map", args);
}

/* ------------------------------------------------------------------ */
/* Firmware — implemented in src-tauri (H5-control):                  */
/* `validate_firmware_image` verifies the Ed25519 signature and the    */
/* image format only; `get_firmware_version` reads the live session's  */
/* `GET_CAPS` reply (errors with no session); `upload_firmware` runs   */
/* the real board OTA path — a fresh session reads the live version,   */
/* the image is verified (signature + format + anti-downgrade vs the   */
/* live version), and the image uploads over HTTP to the board's AP.   */
/*                                                                     */
/* NOTE: `validate_firmware_image` verifies the Ed25519 signature and  */
/* the image format only. Anti-downgrade is enforced by the CALLER    */
/* (the Setup wizard compares the returned `version` against the       */
/* installed version from `get_firmware_version`): the command does    */
/* not know the installed version, so it cannot reject a downgrade    */
/* itself. `upload_firmware` re-validates with the same verifier      */
/* before any upload — the frontend's check is a gate, not a trust    */
/* decision.                                                           */
/* ------------------------------------------------------------------ */

/** Validates a firmware image + Ed25519 signature BEFORE any upload. */
export function validateFirmwareImage(args: {
  path: string;
}): Promise<FirmwareValidation> {
  return invokeCommand<FirmwareValidation>("validate_firmware_image", args);
}

export function uploadFirmware(args: { path: string }): Promise<void> {
  return invokeCommand<void>("upload_firmware", args);
}

export function getFirmwareVersion(): Promise<string> {
  return invokeCommand<string>("get_firmware_version");
}

/* ------------------------------------------------------------------ */
/* Profiles — implemented in src-tauri (Phase G0: AppState).           */
/* ------------------------------------------------------------------ */

export function listProfiles(): Promise<ProfileSummary[]> {
  return invokeCommand<ProfileSummary[]>("list_profiles");
}

export function getActiveProfile(): Promise<ProfileSummary | null> {
  return invokeCommand<ProfileSummary | null>("get_active_profile");
}

export function activateProfile(args: { id: string }): Promise<ProfileSummary> {
  return invokeCommand<ProfileSummary>("activate_profile", args);
}

export function createProfile(args: { name: string }): Promise<ProfileSummary> {
  return invokeCommand<ProfileSummary>("create_profile", args);
}

export function duplicateProfile(args: {
  id: string;
}): Promise<ProfileSummary> {
  return invokeCommand<ProfileSummary>("duplicate_profile", args);
}

export function deleteProfile(args: { id: string }): Promise<void> {
  return invokeCommand<void>("delete_profile", args);
}

/** Import a profile file (size-capped, version-checked by the backend). */
export function importProfile(args: { path: string }): Promise<ProfileSummary> {
  return invokeCommand<ProfileSummary>("import_profile", args);
}

export function exportProfile(args: {
  id: string;
  path: string;
}): Promise<void> {
  return invokeCommand<void>("export_profile", args);
}

/* ------------------------------------------------------------------ */
/* Games — implemented in src-tauri (Phase G0: AppState).               */
/* ------------------------------------------------------------------ */

export function listGames(): Promise<GameInfo[]> {
  return invokeCommand<GameInfo[]>("list_games");
}

export function launchGame(args: { id: string }): Promise<GameInfo> {
  return invokeCommand<GameInfo>("launch_game", args);
}

export function stopGame(args: { id: string }): Promise<GameInfo> {
  return invokeCommand<GameInfo>("stop_game", args);
}

export function getGameState(args: { id: string }): Promise<GameInfo> {
  return invokeCommand<GameInfo>("get_game_state", args);
}

/* ------------------------------------------------------------------ */
/* Settings + data — implemented in src-tauri (Phase G0: AppState),   */
/* plus `open_config_folder` (Phase H3) and `get_startup_notice`      */
/* (Phase H3: the corrupt-store / autostart repair notice).           */
/* ------------------------------------------------------------------ */

export function getSettings(): Promise<Record<string, unknown>> {
  return invokeCommand<Record<string, unknown>>("get_settings");
}

export function setSettings(args: {
  settings: Record<string, unknown>;
}): Promise<Record<string, unknown>> {
  return invokeCommand<Record<string, unknown>>("set_settings", args);
}

export function getAutostart(): Promise<boolean> {
  return invokeCommand<boolean>("get_autostart");
}

export function setAutostart(args: { enabled: boolean }): Promise<boolean> {
  return invokeCommand<boolean>("set_autostart", args);
}

/** Update info from the Tauri updater plugin (Phase F gate). */
export interface UpdateInfo {
  version: string;
  notes: string | null;
  downloadUrl: string | null;
}

/** Asks the updater plugin whether a newer release exists (Phase F gate: implemented). */
export function checkForUpdate(): Promise<UpdateInfo | null> {
  return invokeCommand("check_for_update");
}

/** Exports a diagnostics bundle; resolves with the written path (Phase F gate: implemented). */
export function exportDiagnostics(): Promise<string> {
  return invokeCommand("export_diagnostics");
}

/** One row of the 3-generation backup list (see `BackupInfo`). */
export interface BackupInfo {
  /** Generation (`"current"`, `"bak1"`, `"bak2"`). */
  generation: string;
  /** True when the file for this generation exists on disk. */
  present: boolean;
  /** When the generation was last saved (RFC 3339 UTC), or `null`. */
  savedAt: string | null;
}

/**
 * Forces a backup rotation now and returns the 3-generation list
 * (H3 fix: the Rust command returns `Vec<BackupInfo>`, never a path
 * string — the old `Promise<string>` type rendered the array as text).
 */
export function backupNow(): Promise<BackupInfo[]> {
  return invokeCommand<BackupInfo[]>("backup_now");
}

export function restoreBackup(args: { path: string }): Promise<void> {
  return invokeCommand<void>("restore_backup", args);
}

export function openConfigFolder(): Promise<void> {
  return invokeCommand<void>("open_config_folder");
}

/** 3-generation backup list (Phase F gate: implemented). */
export function listBackups(): Promise<BackupInfo[]> {
  return invokeCommand<BackupInfo[]>("list_backups");
}

/**
 * The startup notice (corrupt-store repair / autostart-repair
 * outcome), or `null` on a clean start. Shown once by the shell.
 */
export function getStartupNotice(): Promise<string | null> {
  return invokeCommand<string | null>("get_startup_notice");
}

/** Windows Firewall rule state (Phase F gate: implemented). */
export function getFirewallState(): Promise<{
  rulePresent: boolean;
  detail: string;
}> {
  return invokeCommand("get_firewall_state");
}

/* ------------------------------------------------------------------ */
/* Calibration + tuning — implemented in src-tauri (Phase G0: AppState). */
/* ------------------------------------------------------------------ */

export function saveCalibration(args: {
  calibration: Calibration;
}): Promise<Calibration> {
  return invokeCommand<Calibration>("save_calibration", args);
}

export function getCalibration(): Promise<Calibration> {
  return invokeCommand<Calibration>("get_calibration");
}

export function getTuning(args: { module: string }): Promise<TuningSettings> {
  return invokeCommand<TuningSettings>("get_tuning", args);
}

export function setTuning(args: TuningSettings): Promise<TuningSettings> {
  return invokeCommand<TuningSettings>("set_tuning", args);
}

/* ------------------------------------------------------------------ */
/* Output backend + engine — implemented in src-tauri (Phase G0:       */
/* AppState); `restart_wheel` implemented in src-tauri (H5-control —  */
/* reboots the board under a fresh session, no auto reconnect).        */
/* ------------------------------------------------------------------ */

/** Output backend state, exactly as the backend reports it (never hardcoded). */
export interface OutputState {
  /** Backend name exactly as the backend reports it (never hardcoded). */
  backend: string;
  connected: boolean;
  detail: string;
  /** Whether output is enabled; absent until the backend reports it. */
  enabled?: boolean;
}

export function getOutputState(): Promise<OutputState> {
  return invokeCommand<OutputState>("get_output_state");
}

/** Output enable/disable (UI plan P6.2 — the ONLY place the output toggle lives). */
export function setOutputState(args: { enabled: boolean }): Promise<OutputState> {
  return invokeCommand<OutputState>("set_output_state", args);
}

export function setOutputBackend(args: { backend: string }): Promise<OutputState> {
  return invokeCommand<OutputState>("set_output_backend", args);
}

/**
 * One output backend as the Output page picker lists it (G4): honest
 * per-backend status from the backend — never hardcoded in the UI.
 * `installed` is `null` only when the probe itself is unavailable.
 */
export interface BackendStatus {
  /** Short id: `"hidmaestro"` / `"vjoy"` / `"none"`. */
  id: string;
  /** Display name exactly as the backend reports it. */
  name: string;
  /** True for the default backend (`HIDMaestro`). */
  isDefault: boolean;
  /** True when this backend is the one selected in settings. */
  selected: boolean;
  /** True when this backend is the live installed backend. */
  active: boolean;
  /** True when the live backend is connected. */
  connected: boolean;
  /** Whether the driver/sidecar file was found by the probe. */
  installed: boolean | null;
  /** True when the backend needs an elevated helper for the session. */
  elevationRequired: boolean;
  /** The UAC copy, when elevation is required. */
  uacNote?: string;
  /** Human-readable status line (shown verbatim). */
  detail: string;
  /** Install/guide link, when the backend is missing (vJoy only). */
  downloadUrl?: string;
}

/** Every selectable backend with honest per-backend status (G4). */
export function listOutputBackends(): Promise<BackendStatus[]> {
  return invokeCommand<BackendStatus[]>("list_output_backends");
}

/** Restarts the wheel (Dashboard only — UI plan golden rule 2). */
export function restartWheel(): Promise<void> {
  return invokeCommand<void>("restart_wheel");
}

export function getEngineStats(): Promise<EngineStats> {
  return invokeCommand<EngineStats>("get_engine_stats");
}

/** Latest snapshot on demand (the live feed is the `rsh://snapshot` event). */
export function getSnapshot(): Promise<EngineSnapshot> {
  return invokeCommand<EngineSnapshot>("get_snapshot");
}

/* ------------------------------------------------------------------ */
/* Events (backend -> frontend).                                       */
/*                                                                     */
/* The 30 Hz snapshot feed is an event, not a command: the engine      */
/* pushes, the UI reads. Subscribe once per component and unsubscribe  */
/* on unmount. `rsh://snapshot` and `rsh://button-press` ARE emitted  */
/* by src-tauri now (Phase G0 emitter thread); `rsh://game-state`,     */
/* `rsh://config-rollback` and `rsh://pairing-window` are NOT YET     */
/* EMITTED (no watcher / staging / pairing owner yet).                 */
/* ------------------------------------------------------------------ */

/** Subscribe to the 30 Hz engine snapshot feed (`rsh://snapshot`). */
export async function subscribeEngineSnapshot(
  handler: (snapshot: EngineSnapshot) => void,
): Promise<UnlistenFn> {
  return listen<EngineSnapshot>("rsh://snapshot", (event) => {
    handler(event.payload);
  });
}

/** Button-press highlight events (target < 50 ms to highlight). */
export async function subscribeButtonPress(
  handler: (payload: { button: number; pressed: boolean }) => void,
): Promise<UnlistenFn> {
  return listen<{ button: number; pressed: boolean }>(
    "rsh://button-press",
    (event) => {
      handler(event.payload);
    },
  );
}

/** Game running/stopped state changes. */
export async function subscribeGameState(
  handler: (game: GameInfo) => void,
): Promise<UnlistenFn> {
  return listen<GameInfo>("rsh://game-state", (event) => {
    handler(event.payload);
  });
}

/** Config rollback (e.g. confirm window expired) notifications. */
export async function subscribeConfigRollback(
  handler: (payload: { reason: string }) => void,
): Promise<UnlistenFn> {
  return listen<{ reason: string }>("rsh://config-rollback", (event) => {
    handler(event.payload);
  });
}

/** Pairing-window countdown / expiry notifications. */
export async function subscribePairingWindow(
  handler: (status: PairingStatus) => void,
): Promise<UnlistenFn> {
  return listen<PairingStatus>("rsh://pairing-window", (event) => {
    handler(event.payload);
  });
}
