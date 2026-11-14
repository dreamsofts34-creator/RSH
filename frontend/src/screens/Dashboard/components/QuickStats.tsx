import { useTranslation } from "react-i18next";
import type { EngineStats } from "@/lib/tauri";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";

/**
 * Format one backend-reported stat.
 * A null value (or 0) means "unmeasured" for rates/latencies — the
 * backend never reports a real 0 ms latency — so it renders as
 * "not measured", never as a fake 0. Packet loss is the exception: 0 %
 * is a real, measured value.
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

export function formatLossPct(lossPct: number): string {
  return `${lossPct.toFixed(1)} %`;
}

interface QuickStatsProps {
  /** Snapshot stats win when live; otherwise the polled engine stats. */
  stats: EngineStats | null;
}

/** Quick stats — renders only numbers the backend reported. */
export function QuickStats({ stats }: QuickStatsProps) {
  const { t } = useTranslation();
  const notMeasured = t("dashboard.stats.notMeasured");

  const rows: { label: string; value: string }[] = stats
    ? [
        {
          label: t("dashboard.stats.loopRate"),
          value: formatStat(stats.loopHz, "Hz", notMeasured),
        },
        {
          label: t("dashboard.stats.loss"),
          value: formatLossPct(stats.lossPct),
        },
        {
          label: t("dashboard.stats.jitter"),
          value: formatStat(stats.jitterMsP99, "ms", notMeasured),
        },
        {
          label: t("dashboard.stats.outputLatency"),
          value: formatStat(stats.outputLatencyMs, "ms", notMeasured),
        },
        {
          label: t("dashboard.stats.e2e"),
          value: formatStat(stats.e2eMs, "ms", notMeasured),
        },
      ]
    : [];

  return (
    <Card>
      <CardHeader>
        <CardTitle>{t("dashboard.stats.title")}</CardTitle>
      </CardHeader>
      <CardContent>
        {stats ? (
          <dl className="grid grid-cols-2 gap-x-4 gap-y-3 sm:grid-cols-3 lg:grid-cols-5">
            {rows.map((row) => (
              <div key={row.label} className="space-y-0.5">
                <dt className="text-xs text-[var(--muted)]">{row.label}</dt>
                <dd className="font-mono text-sm font-semibold">{row.value}</dd>
              </div>
            ))}
          </dl>
        ) : (
          <p className="text-sm text-[var(--muted)]">
            {t("dashboard.stats.notAvailable")}
          </p>
        )}
      </CardContent>
    </Card>
  );
}
