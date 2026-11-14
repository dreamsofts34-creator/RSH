import { useTranslation } from "react-i18next";
import type { EngineStats } from "@/lib/tauri";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";

/**
 * Same rule as the Dashboard quick stats: a null rate/latency means
 * "unmeasured" and renders as "not measured", never as a fake 0 — and
 * neither does a 0 (the backend never reports a real 0 ms latency).
 * Packet loss 0 % is real and shown as 0.0 %.
 */
export function formatStat(
  value: number | null,
  unit: string,
  notMeasured: string,
): string {
  if (value === null || value <= 0) return notMeasured;
  const shown = Number.isInteger(value) ? String(value) : value.toFixed(1);
  return `${shown} ${unit}`;
}

interface PacketStatsProps {
  stats: EngineStats | null;
  seqGaps: number;
}

/** Packet stats from the live snapshot stats (backend-reported only). */
export function PacketStats({ stats, seqGaps }: PacketStatsProps) {
  const { t } = useTranslation();
  const notMeasured = t("monitor.stats.notMeasured");

  return (
    <Card>
      <CardHeader>
        <CardTitle>{t("monitor.stats.title")}</CardTitle>
      </CardHeader>
      <CardContent>
        {stats ? (
          <dl className="grid grid-cols-2 gap-x-4 gap-y-3 sm:grid-cols-3">
            <div className="space-y-0.5">
              <dt className="text-xs text-[var(--muted)]">
                {t("monitor.stats.loopRate")}
              </dt>
              <dd className="font-mono text-sm font-semibold">
                {formatStat(stats.loopHz, "Hz", notMeasured)}
              </dd>
            </div>
            <div className="space-y-0.5">
              <dt className="text-xs text-[var(--muted)]">
                {t("monitor.stats.loss")}
              </dt>
              <dd className="font-mono text-sm font-semibold">
                {stats.lossPct.toFixed(1)} %
              </dd>
            </div>
            <div className="space-y-0.5">
              <dt className="text-xs text-[var(--muted)]">
                {t("monitor.stats.jitter")}
              </dt>
              <dd className="font-mono text-sm font-semibold">
                {formatStat(stats.jitterMsP99, "ms", notMeasured)}
              </dd>
            </div>
            <div className="space-y-0.5">
              <dt className="text-xs text-[var(--muted)]">
                {t("monitor.stats.outputLatency")}
              </dt>
              <dd className="font-mono text-sm font-semibold">
                {formatStat(stats.outputLatencyMs, "ms", notMeasured)}
              </dd>
            </div>
            <div className="space-y-0.5">
              <dt className="text-xs text-[var(--muted)]">
                {t("monitor.stats.e2e")}
              </dt>
              <dd className="font-mono text-sm font-semibold">
                {formatStat(stats.e2eMs, "ms", notMeasured)}
              </dd>
            </div>
            <div className="space-y-0.5">
              <dt className="text-xs text-[var(--muted)]">
                {t("monitor.stats.seqGaps")}
              </dt>
              <dd className="font-mono text-sm font-semibold">{seqGaps}</dd>
            </div>
          </dl>
        ) : (
          <p className="text-sm text-[var(--muted)]">
            {t("monitor.stats.notAvailable")}
          </p>
        )}
      </CardContent>
    </Card>
  );
}
