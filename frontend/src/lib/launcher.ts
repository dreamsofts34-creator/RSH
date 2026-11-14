/**
 * Game launcher (SW-R7.3) — thin invoke wrappers for the Dashboard.
 *
 * This module only re-exports the game commands from `./tauri` and adds
 * pure helpers over the `GameInfo` shape. It owns no backend logic and
 * no screen code: the Rust side lives in `crates/rsh-app-core/src/launcher.rs`
 * and the Tauri command registration in `src-tauri/src/main.rs`.
 *
 * The `list_games` / `launch_game` / `stop_game` / `get_game_state`
 * commands are NOT YET IMPLEMENTED in src-tauri (Phase E gate) — see
 * `NOT_YET_IMPLEMENTED_COMMANDS` in `./tauri`. Until the Phase F gate
 * registers them, every call below fails loudly as `TauriCommandError`
 * with `code: "NOT_IMPLEMENTED"`. Callers must treat that as "backend
 * feature not built yet", never as data.
 */

import {
  getGameState,
  launchGame,
  listGames,
  stopGame,
  subscribeGameState,
  type GameInfo,
  TauriCommandError,
} from "./tauri";

export {
  getGameState,
  launchGame,
  listGames,
  stopGame,
  subscribeGameState,
  type GameInfo,
  TauriCommandError,
};

/** Tauri commands this module uses (must match `src-tauri/src/main.rs`). */
export const LAUNCHER_COMMANDS: readonly string[] = [
  "list_games",
  "launch_game",
  "stop_game",
  "get_game_state",
];

/**
 * Games the user can launch right now: installed and not already running.
 * Pure function over the backend-reported list — never invents a game.
 */
export function getLaunchableGames(games: readonly GameInfo[]): GameInfo[] {
  return games.filter((game) => game.installed && !game.running);
}

/** The currently running game, if any (at most one runs at a time). */
export function getRunningGame(
  games: readonly GameInfo[],
): GameInfo | null {
  return games.find((game) => game.running) ?? null;
}

/** Whether the Dashboard's Launch button should be enabled for a game. */
export function canLaunch(game: GameInfo): boolean {
  return game.installed && !game.running;
}
