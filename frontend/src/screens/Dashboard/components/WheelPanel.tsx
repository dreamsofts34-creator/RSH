import { memo } from "react";
import { useTranslation } from "react-i18next";
import type { EngineSnapshot } from "@/lib/tauri";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Progress } from "@/components/ui/progress";

interface WheelPanelProps {
  snapshot: EngineSnapshot | null;
}

/** Clamp the wheel rotation to the 0–900° range (±450°). */
function clampAngle(deg: number): number {
  return Math.max(-450, Math.min(450, deg));
}

function PedalRow({
  label,
  value,
}: {
  label: string;
  value: number;
}) {
  const pct = Math.round(Math.max(0, Math.min(1, value)) * 100);
  return (
    <div className="space-y-1">
      <div className="flex items-center justify-between text-[13px]">
        <span className="text-[var(--muted)]">{label}</span>
        <span className="font-mono font-medium">{pct} %</span>
      </div>
      <Progress value={pct} aria-label={`${label}: ${pct}%`} />
    </div>
  );
}

/**
 * Steering wheel visual (rotation = live angle) + pedal bars.
 * The clutch bar renders only when the snapshot carries a clutch channel
 * (a null clutch means the board has no clutch — never a fake bar).
 */
function WheelPanelInner({ snapshot }: WheelPanelProps) {
  const { t } = useTranslation();
  const angle = snapshot ? clampAngle(snapshot.steeringDeg) : 0;

  return (
    <Card>
      <CardHeader>
        <CardTitle>{t("dashboard.wheel.title")}</CardTitle>
      </CardHeader>
      <CardContent className="flex flex-col items-center gap-5 sm:flex-row sm:items-start sm:gap-8">
        <div className="flex shrink-0 flex-col items-center gap-2">
          <div
            aria-label={`${t("dashboard.wheel.steeringAngle")}: ${Math.round(angle)}°`}
            className="relative h-36 w-36 rounded-full border-4 border-[var(--line)] bg-[var(--raised)]"
          >
            <div
              aria-hidden="true"
              className="absolute inset-0 transition-transform duration-100 ease-linear"
              style={{ transform: `rotate(${angle}deg)` }}
            >
              <span className="absolute left-1/2 top-2 h-5 w-1.5 -translate-x-1/2 rounded-full bg-[var(--accent)]" />
              <span className="absolute left-1/2 top-1/2 h-1.5 w-1.5 -translate-x-1/2 -translate-y-1/2 rounded-full bg-[var(--muted)]" />
            </div>
          </div>
          <p className="font-mono text-sm font-semibold">
            {Math.round(angle)}°
          </p>
        </div>

        <div className="w-full min-w-0 flex-1 space-y-4">
          <p className="text-sm font-semibold">
            {t("dashboard.wheel.pedals")}
          </p>
          {snapshot ? (
            <>
              <PedalRow
                label={t("dashboard.wheel.accel")}
                value={snapshot.pedals.accel}
              />
              <PedalRow
                label={t("dashboard.wheel.brake")}
                value={snapshot.pedals.brake}
              />
              {snapshot.pedals.clutch !== null ? (
                <PedalRow
                  label={t("dashboard.wheel.clutch")}
                  value={snapshot.pedals.clutch}
                />
              ) : null}
            </>
          ) : (
            <p className="text-sm text-[var(--muted)]">
              {t("dashboard.wheel.noSnapshot")}
            </p>
          )}
        </div>
      </CardContent>
    </Card>
  );
}

/** Memoised: the Dashboard re-renders at 30 Hz from the snapshot feed
 *  (H4) — this component only re-renders when its own props change. */
export const WheelPanel = memo(WheelPanelInner);
