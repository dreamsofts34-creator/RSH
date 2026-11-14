import { useEffect, useRef, useState, type RefObject } from "react";
import { useTranslation } from "react-i18next";
import type uPlot from "uplot";
import "uplot/dist/uPlot.min.css";
import type { EngineSnapshot } from "@/lib/tauri";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";

/**
 * Live uPlot charts (UI plan P8.2): steering angle + pedals over time.
 *
 * Performance rules (R-P6 / R-P7):
 * - Ingest is 30 Hz (the snapshot ref), redraw is capped at 10 Hz.
 * - Ring buffers are bounded (300 points = 10 s); old points drop off
 *   the front, nothing grows without bound.
 * - No per-frame allocation in the hot path beyond the small push/shift
 *   on the fixed-size buffers.
 * - Theme changes re-create the charts with colours re-read from the
 *   CSS variables, so a theme switch re-themes the charts without a
 *   page reload.
 *
 * The clutch series carries `null` while the snapshot has no clutch
 * channel — a missing channel renders as a gap, never as a fake line.
 * Where the environment has no canvas (jsdom tests) chart creation is
 * skipped and the live numeric readouts still update.
 */

const CAPACITY = 300;
const REDRAW_MS = 100;

interface RingBuffers {
  xs: number[];
  steering: number[];
  accel: number[];
  brake: number[];
  clutch: (number | null)[];
  t0: number | null;
}

function createBuffers(): RingBuffers {
  return { xs: [], steering: [], accel: [], brake: [], clutch: [], t0: null };
}

function pushPoint(buffers: RingBuffers, snap: EngineSnapshot): void {
  const now = performance.now() / 1000;
  if (buffers.t0 === null) buffers.t0 = now;
  buffers.xs.push(now - buffers.t0);
  buffers.steering.push(snap.steeringDeg);
  buffers.accel.push(snap.pedals.accel);
  buffers.brake.push(snap.pedals.brake);
  buffers.clutch.push(snap.pedals.clutch);
  if (buffers.xs.length > CAPACITY) {
    buffers.xs.shift();
    buffers.steering.shift();
    buffers.accel.shift();
    buffers.brake.shift();
    buffers.clutch.shift();
  }
}

interface ChartColors {
  accent: string;
  success: string;
  info: string;
  muted: string;
  ink: string;
  line: string;
}

function readColors(): ChartColors {
  const cs = getComputedStyle(document.documentElement);
  const pick = (name: string, fallback: string): string =>
    cs.getPropertyValue(name).trim() || fallback;
  return {
    accent: pick("--accent", "violet"),
    success: pick("--success", "green"),
    info: pick("--info", "cyan"),
    muted: pick("--muted", "gray"),
    ink: pick("--ink", "black"),
    line: pick("--line", "lightgray"),
  };
}

interface LiveChartsProps {
  snapshotRef: RefObject<EngineSnapshot | null>;
}

interface LegendValues {
  steering: number | null;
  accel: number | null;
  brake: number | null;
  clutch: number | null;
}

export function LiveCharts({ snapshotRef }: LiveChartsProps) {
  const { t } = useTranslation();
  const steeringHostRef = useRef<HTMLDivElement | null>(null);
  const pedalsHostRef = useRef<HTMLDivElement | null>(null);
  const chartsRef = useRef<{ steering: uPlot | null; pedals: uPlot | null }>({
    steering: null,
    pedals: null,
  });
  // uPlot is loaded lazily: its module evaluates browser APIs
  // (matchMedia) at import time, which jsdom lacks, and the charts are
  // optional — a failed load degrades to the numeric readouts, never a
  // crash. In the Tauri WebView the import always succeeds.
  const uPlotCtorRef = useRef<typeof uPlot | null>(null);
  const buffersRef = useRef<RingBuffers>(createBuffers());
  const [unavailable, setUnavailable] = useState(false);
  const [legend, setLegend] = useState<LegendValues>({
    steering: null,
    accel: null,
    brake: null,
    clutch: null,
  });

  useEffect(() => {
    let cancelled = false;

    const buildCharts = async (): Promise<void> => {
      const steeringHost = steeringHostRef.current;
      const pedalsHost = pedalsHostRef.current;
      if (!steeringHost || !pedalsHost) return;
      const width = steeringHost.clientWidth;
      // No layout width (jsdom) or no canvas: charts cannot be built.
      if (width <= 0 || typeof document === "undefined") {
        if (!cancelled) setUnavailable(true);
        return;
      }
      let UPlot = uPlotCtorRef.current;
      if (!UPlot) {
        try {
          UPlot = (await import("uplot")).default;
          uPlotCtorRef.current = UPlot;
        } catch {
          if (!cancelled) setUnavailable(true);
          return;
        }
      }
      if (cancelled) return;
      const colors = readColors();
      const buffers = buffersRef.current;
      const axes = [
        {
          stroke: colors.muted,
          grid: { stroke: colors.line },
          ticks: { stroke: colors.line },
        },
        { stroke: colors.muted, grid: { stroke: colors.line } },
      ];
      try {
        chartsRef.current.steering?.destroy();
        chartsRef.current.pedals?.destroy();
        chartsRef.current.steering = new UPlot(
          {
            width,
            height: 180,
            series: [
              {},
              {
                label: "Steering",
                stroke: colors.accent,
                width: 2,
              },
            ],
            axes,
          },
          [buffers.xs, buffers.steering],
          steeringHost,
        );
        chartsRef.current.pedals = new UPlot(
          {
            width,
            height: 180,
            series: [
              {},
              { label: "Accel", stroke: colors.accent, width: 2 },
              { label: "Brake", stroke: colors.success, width: 2 },
              { label: "Clutch", stroke: colors.info, width: 2 },
            ],
            axes: [
              axes[0],
              {
                stroke: colors.muted,
                grid: { stroke: colors.line },
                values: (_u, vals) =>
                  vals.map((v) => (v === null ? "" : `${Math.round(v * 100)}%`)),
              },
            ],
          },
          [buffers.xs, buffers.accel, buffers.brake, buffers.clutch],
          pedalsHost,
        );
        if (!cancelled) setUnavailable(false);
      } catch {
        if (!cancelled) setUnavailable(true);
      }
    };

    void buildCharts();

    // Re-theme the charts when the theme flips (R-P10: no page reload).
    const observer = new MutationObserver(() => {
      void buildCharts();
    });
    observer.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ["data-theme", "class"],
    });

    // 10 Hz redraw: pull the latest snapshot from the ref, push one
    // point, redraw. The 30 Hz ingest never touches React state.
    const timer = window.setInterval(() => {
      const snap = snapshotRef.current;
      if (!snap) return;
      pushPoint(buffersRef.current, snap);
      const buffers = buffersRef.current;
      chartsRef.current.steering?.setData([buffers.xs, buffers.steering]);
      chartsRef.current.pedals?.setData([
        buffers.xs,
        buffers.accel,
        buffers.brake,
        buffers.clutch,
      ]);
      setLegend({
        steering: snap.steeringDeg,
        accel: snap.pedals.accel,
        brake: snap.pedals.brake,
        clutch: snap.pedals.clutch,
      });
    }, REDRAW_MS);

    return () => {
      cancelled = true;
      window.clearInterval(timer);
      observer.disconnect();
      chartsRef.current.steering?.destroy();
      chartsRef.current.pedals?.destroy();
      chartsRef.current = { steering: null, pedals: null };
    };
  }, [snapshotRef]);

  const fmtPct = (v: number | null): string =>
    v === null ? "—" : `${Math.round(v * 100)} %`;

  return (
    <Card>
      <CardHeader>
        <CardTitle>{t("monitor.charts.title")}</CardTitle>
      </CardHeader>
      <CardContent className="space-y-5">
        {unavailable ? (
          <p className="text-sm text-[var(--muted)]">
            {t("monitor.charts.unavailable")}
          </p>
        ) : null}
        <div>
          <div className="mb-1 flex items-center justify-between text-sm">
            <span className="font-semibold">
              {t("monitor.charts.steering")}
            </span>
            <span className="font-mono">
              {legend.steering === null
                ? "—"
                : `${Math.round(legend.steering)}°`}
            </span>
          </div>
          <div ref={steeringHostRef} aria-label={t("monitor.charts.steering")} />
        </div>
        <div>
          <div className="mb-1 flex flex-wrap items-center justify-between gap-2 text-sm">
            <span className="font-semibold">
              {t("monitor.charts.pedals")}
            </span>
            <span className="flex gap-3 font-mono text-[13px]">
              <span>
                {t("monitor.charts.accel")}: {fmtPct(legend.accel)}
              </span>
              <span>
                {t("monitor.charts.brake")}: {fmtPct(legend.brake)}
              </span>
              <span>
                {t("monitor.charts.clutch")}: {fmtPct(legend.clutch)}
              </span>
            </span>
          </div>
          <div ref={pedalsHostRef} aria-label={t("monitor.charts.pedals")} />
        </div>
        {legend.steering === null ? (
          <p className="text-sm text-[var(--muted)]">
            {t("monitor.charts.noData")}
          </p>
        ) : null}
      </CardContent>
    </Card>
  );
}
