import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Gauge } from "lucide-react";
import {
  TauriCommandError,
  getActiveProfile,
  getEngineStats,
  getTransportState,
  launchGame,
  listGames,
  listProfiles,
  restartWheel,
  stopGame,
  subscribeGameState,
  type EngineStats,
  type GameInfo,
  type ProfileSummary,
  type TransportState,
} from "@/lib/tauri";
import { ConnectionCard, type ConnectionLoadState } from "./components/ConnectionCard";
import { WheelPanel } from "./components/WheelPanel";
import { QuickStats } from "./components/QuickStats";
import { GameLauncher, type GameLoadState } from "./components/GameLauncher";
import { RestartCard, type RestartState } from "./components/RestartCard";
import { useEngineSnapshot } from "./useSnapshot";

export const path = "dashboard";
export const label = "Dashboard";
export const icon = Gauge;
export const component = DashboardScreen;

/** Map a load failure onto the honest UI state for that section. */
function loadStateOf(error: unknown): ConnectionLoadState {
  if (error instanceof TauriCommandError && error.notImplemented) {
    return "notImplemented";
  }
  return "error";
}

/**
 * Dashboard (UI plan P3): connection card, live wheel + pedals, engine
 * quick stats, game launcher, and the Restart wheel button.
 *
 * Every section loads independently: one backend command being
 * NOT_IMPLEMENTED never blanks the others. No section invents data —
 * missing stats render as "not available", an unmeasured latency as
 * "not measured", and an unconfigured game as "not configured".
 */
export function DashboardScreen() {
  const { t } = useTranslation();
  const snapshot = useEngineSnapshot();

  const [transport, setTransport] = useState<TransportState | null>(null);
  const [transportLoad, setTransportLoad] =
    useState<ConnectionLoadState | null>("loading");
  const [games, setGames] = useState<GameInfo[] | null>(null);
  const [gamesLoad, setGamesLoad] = useState<GameLoadState | null>("loading");
  const [profiles, setProfiles] = useState<ProfileSummary[]>([]);
  const [activeProfile, setActiveProfile] = useState<ProfileSummary | null>(
    null,
  );
  const [engineStats, setEngineStats] = useState<EngineStats | null>(null);
  const [launchingId, setLaunchingId] = useState<string | null>(null);
  const [stoppingId, setStoppingId] = useState<string | null>(null);
  const [gameError, setGameError] = useState<string | null>(null);
  const [restartState, setRestartState] = useState<RestartState>("idle");
  // M8: profiles / active profile / stats failures are shown, not
  // swallowed — each section reports its own load failure.
  const [profilesFailed, setProfilesFailed] = useState(false);
  const [activeFailed, setActiveFailed] = useState(false);
  const [statsFailed, setStatsFailed] = useState(false);
  // M8 staleness: the last time a snapshot arrived (null = never).
  const [stale, setStale] = useState(false);
  const lastSnapshotAtRef = useRef(0);
  const restartTimerRef = useRef<number | undefined>(undefined);

  // Load: transport, games, profiles, active profile, engine stats.
  // Re-polled every 30 s (M8) so a failed section recovers on its own
  // and the launcher/profile data never goes silently stale.
  const loadAll = useCallback(async () => {
    const [transportResult, gamesResult, profilesResult, activeResult, statsResult] =
      await Promise.allSettled([
        getTransportState(),
        listGames(),
        listProfiles(),
        getActiveProfile(),
        getEngineStats(),
      ]);
    if (transportResult.status === "fulfilled") {
      setTransport(transportResult.value);
      setTransportLoad(null);
    } else {
      setTransportLoad(loadStateOf(transportResult.reason));
    }
    if (gamesResult.status === "fulfilled") {
      setGames(gamesResult.value);
      setGamesLoad(null);
    } else {
      setGamesLoad(loadStateOf(gamesResult.reason));
    }
    if (profilesResult.status === "fulfilled") {
      setProfiles(profilesResult.value);
      setProfilesFailed(false);
    } else {
      setProfilesFailed(true);
    }
    if (activeResult.status === "fulfilled") {
      setActiveProfile(activeResult.value);
      setActiveFailed(false);
    } else {
      setActiveFailed(true);
    }
    if (statsResult.status === "fulfilled") {
      setEngineStats(statsResult.value);
      setStatsFailed(false);
    } else {
      setStatsFailed(true);
    }
  }, []);

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      if (!cancelled) await loadAll();
    })();
    const timer = window.setInterval(() => {
      void loadAll();
    }, 30_000);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [loadAll]);

  // Staleness check: no snapshot for >3 s means the live feed is stale.
  useEffect(() => {
    if (snapshot) lastSnapshotAtRef.current = Date.now();
  }, [snapshot]);

  useEffect(() => {
    const timer = window.setInterval(() => {
      setStale(
        lastSnapshotAtRef.current !== 0 &&
          Date.now() - lastSnapshotAtRef.current > 3000,
      );
    }, 1000);
    return () => window.clearInterval(timer);
  }, []);

  // Game running/stopped changes arrive on `rsh://game-state`.
  useEffect(() => {
    let done = false;
    let unlisten: (() => void) | undefined;
    void subscribeGameState((game) => {
      if (done) return;
      setGames((prev) =>
        prev ? prev.map((g) => (g.id === game.id ? game : g)) : prev,
      );
    })
      .then((stop) => {
        if (done) stop();
        else unlisten = stop;
      })
      .catch(() => {
        // Event not available yet; the launcher still works via commands.
      });
    return () => {
      done = true;
      unlisten?.();
    };
  }, []);

  const handleLaunch = useCallback(async (game: GameInfo) => {
    setLaunchingId(game.id);
    setGameError(null);
    try {
      const updated = await launchGame({ id: game.id });
      setGames((prev) =>
        prev ? prev.map((g) => (g.id === updated.id ? updated : g)) : prev,
      );
    } catch (error) {
      if (error instanceof TauriCommandError && error.notImplemented) {
        setGameError("dashboard.games.notAvailable");
      } else {
        setGameError("dashboard.games.launchFailed");
      }
    } finally {
      setLaunchingId(null);
    }
  }, []);

  const handleStop = useCallback(async (game: GameInfo) => {
    setStoppingId(game.id);
    setGameError(null);
    try {
      const updated = await stopGame({ id: game.id });
      setGames((prev) =>
        prev ? prev.map((g) => (g.id === updated.id ? updated : g)) : prev,
      );
    } catch (error) {
      if (error instanceof TauriCommandError && error.notImplemented) {
        setGameError("dashboard.games.notAvailable");
      } else {
        setGameError("dashboard.games.stopFailed");
      }
    } finally {
      setStoppingId(null);
    }
  }, []);

  // M4: the "done" state resets after 8 s so the button is usable
  // again — a stuck "done" used to hide the reboot guidance forever.
  useEffect(() => {
    if (restartState !== "done") return undefined;
    restartTimerRef.current = window.setTimeout(() => {
      setRestartState("idle");
    }, 8000);
    return () => window.clearTimeout(restartTimerRef.current);
  }, [restartState]);

  const handleRestart = useCallback(async () => {
    setRestartState("busy");
    try {
      await restartWheel();
      setRestartState("done");
    } catch (error) {
      if (error instanceof TauriCommandError && error.notImplemented) {
        setRestartState("notImplemented");
      } else {
        setRestartState("error");
      }
    }
  }, []);

  // Live snapshot stats win; the polled stats are the fallback.
  const stats = snapshot?.stats ?? engineStats;

  return (
    <section aria-label={t("dashboard.title")} className="space-y-5">
      <header>
        <h1 className="text-[26px] font-bold leading-tight">
          {t("dashboard.title")}
        </h1>
        <p className="mt-1 text-sm text-[var(--muted)]">
          {t("dashboard.subtitle")}
        </p>
        {stale ? (
          <p role="status" className="mt-1 text-sm text-[var(--warning-text)]">
            {t("dashboard.stale")}
          </p>
        ) : null}
        {profilesFailed || activeFailed || statsFailed ? (
          <p role="alert" className="mt-1 text-sm text-[var(--error-text)]">
            {[
              profilesFailed ? t("dashboard.profilesLoadFailed") : null,
              activeFailed ? t("dashboard.activeLoadFailed") : null,
              statsFailed ? t("dashboard.statsLoadFailed") : null,
            ]
              .filter(Boolean)
              .join(" ")}
          </p>
        ) : null}
      </header>

      <div className="grid gap-5 lg:grid-cols-3">
        <ConnectionCard
          transport={transport}
          loadState={transportLoad}
          liveConnection={snapshot?.connection ?? null}
        />
        <RestartCard
          activeProfile={activeProfile}
          restartState={restartState}
          onRestart={handleRestart}
        />
        <div className="lg:col-span-1">
          <WheelPanel snapshot={snapshot} />
        </div>
      </div>

      <QuickStats stats={stats} />

      <GameLauncher
        games={games}
        loadState={gamesLoad}
        profiles={profiles}
        launchingId={launchingId}
        stoppingId={stoppingId}
        error={gameError ? t(gameError) : null}
        onLaunch={handleLaunch}
        onStop={handleStop}
      />
    </section>
  );
}
