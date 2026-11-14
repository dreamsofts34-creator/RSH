import { useTranslation } from "react-i18next";
import { Gamepad2 } from "lucide-react";
import type { GameInfo, ProfileSummary } from "@/lib/tauri";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";

export type GameLoadState = "loading" | "notImplemented" | "error";

interface GameLauncherProps {
  games: GameInfo[] | null;
  loadState: GameLoadState | null;
  profiles: ProfileSummary[];
  launchingId: string | null;
  stoppingId: string | null;
  error: string | null;
  onLaunch: (game: GameInfo) => void;
  onStop: (game: GameInfo) => void;
}

/**
 * Game launcher (UI plan P3.3). One game runs at a time: while one is
 * running every other Launch is disabled with the reason shown.
 * A game with no bound profile says so honestly ("Not configured") and its
 * Launch still works — it is never a dead button. A game the backend
 * reports as not installed cannot be launched.
 */
export function GameLauncher({
  games,
  loadState,
  profiles,
  launchingId,
  stoppingId,
  error,
  onLaunch,
  onStop,
}: GameLauncherProps) {
  const { t } = useTranslation();
  const runningGame = games?.find((g) => g.running) ?? null;

  return (
    <Card>
      <CardHeader className="flex flex-row items-center justify-between space-y-0">
        <CardTitle>{t("dashboard.games.title")}</CardTitle>
        <Gamepad2 className="h-4 w-4 text-[var(--muted)]" aria-hidden="true" />
      </CardHeader>
      <CardContent className="space-y-3">
        {loadState === "loading" && !games ? (
          <p className="text-sm text-[var(--muted)]">
            {t("dashboard.loading")}
          </p>
        ) : !games ? (
          <p className="text-sm text-[var(--muted)]">
            {t("dashboard.games.notAvailable")}
          </p>
        ) : games.length === 0 ? (
          <p className="text-sm text-[var(--muted)]">
            {t("dashboard.games.notAvailable")}
          </p>
        ) : (
          <ul className="space-y-3">
            {games.map((game) => {
              const boundProfile = game.boundProfileId
                ? (profiles.find((p) => p.id === game.boundProfileId) ?? null)
                : null;
              const blockedByRunning =
                runningGame !== null && runningGame.id !== game.id;
              const isLaunching = launchingId === game.id;
              const isStopping = stoppingId === game.id;
              return (
                <li
                  key={game.id}
                  className="flex flex-col gap-2 rounded-xl border border-[var(--line)] p-3 sm:flex-row sm:items-center sm:justify-between"
                >
                  <div className="min-w-0">
                    <div className="flex flex-wrap items-center gap-2">
                      <p className="truncate text-sm font-semibold">
                        {game.name}
                      </p>
                      {game.running ? (
                        <Badge variant="success">
                          {t("dashboard.games.running")}
                        </Badge>
                      ) : !game.installed ? (
                        <Badge variant="secondary">
                          {t("dashboard.games.notInstalled")}
                        </Badge>
                      ) : null}
                    </div>
                    <p className="mt-0.5 text-[13px] text-[var(--muted)]">
                      {t("dashboard.games.boundProfile")}:{" "}
                      {game.boundProfileId === null
                        ? t("dashboard.games.noProfile")
                        : (boundProfile?.name ?? game.boundProfileId)}
                      {game.boundProfileId === null ? (
                        <>
                          {" — "}
                          <span>{t("dashboard.games.notConfigured")}</span>
                        </>
                      ) : (
                        ""
                      )}
                    </p>
                    {blockedByRunning ? (
                      <p className="mt-0.5 text-[13px] text-[var(--muted)]">
                        {t("dashboard.games.stopRunningFirst")}
                      </p>
                    ) : null}
                  </div>
                  <div className="flex shrink-0 gap-2">
                    {game.running ? (
                      <Button
                        type="button"
                        variant="outline"
                        size="sm"
                        disabled={isStopping}
                        onClick={() => onStop(game)}
                      >
                        {isStopping
                          ? t("dashboard.games.stopping")
                          : t("dashboard.games.stop")}
                      </Button>
                    ) : (
                      <Button
                        type="button"
                        size="sm"
                        disabled={
                          !game.installed || blockedByRunning || isLaunching
                        }
                        onClick={() => onLaunch(game)}
                      >
                        {isLaunching
                          ? t("dashboard.games.launching")
                          : t("dashboard.games.launch")}
                      </Button>
                    )}
                  </div>
                </li>
              );
            })}
          </ul>
        )}
        {error ? (
          <p role="alert" className="text-sm text-[var(--error-text)]">
            {error}
          </p>
        ) : null}
      </CardContent>
    </Card>
  );
}
