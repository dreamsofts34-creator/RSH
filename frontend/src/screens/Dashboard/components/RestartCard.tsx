import { useTranslation } from "react-i18next";
import { RotateCcw } from "lucide-react";
import type { ProfileSummary } from "@/lib/tauri";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Button } from "@/components/ui/button";

export type RestartState = "idle" | "busy" | "done" | "notImplemented" | "error";

interface RestartCardProps {
  activeProfile: ProfileSummary | null;
  restartState: RestartState;
  onRestart: () => void;
}

/**
 * Active profile + Restart wheel. This is the ONLY place the Restart
 * wheel control lives (UI plan golden rule 2).
 */
export function RestartCard({
  activeProfile,
  restartState,
  onRestart,
}: RestartCardProps) {
  const { t } = useTranslation();

  return (
    <Card>
      <CardHeader>
        <CardTitle>{t("dashboard.restart.title")}</CardTitle>
      </CardHeader>
      <CardContent className="space-y-3">
        <div className="flex items-center justify-between gap-2 text-sm">
          <span className="text-[var(--muted)]">
            {t("dashboard.profile.active")}
          </span>
          <span className="font-medium">
            {activeProfile ? activeProfile.name : t("dashboard.profile.none")}
          </span>
        </div>
        <Button
          type="button"
          variant="outline"
          className="w-full"
          disabled={restartState === "busy"}
          onClick={onRestart}
        >
          <RotateCcw className="h-4 w-4" aria-hidden="true" />
          {restartState === "busy"
            ? t("dashboard.restart.restarting")
            : t("dashboard.restart.button")}
        </Button>
        {restartState === "done" ? (
          <p role="status" className="text-sm text-[var(--success-text)]">
            {t("dashboard.restart.done")}
          </p>
        ) : null}
        {restartState === "error" ? (
          <p role="alert" className="text-sm text-[var(--error-text)]">
            {t("dashboard.restart.failed")}
          </p>
        ) : null}
        {restartState === "notImplemented" ? (
          <p role="alert" className="text-sm text-[var(--muted)]">
            {t("dashboard.restart.notAvailable")}
          </p>
        ) : null}
      </CardContent>
    </Card>
  );
}
