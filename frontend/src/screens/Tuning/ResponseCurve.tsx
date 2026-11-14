import { useEffect, useMemo, useRef, useState } from "react";
import type uPlot from "uplot";
import "uplot/dist/uPlot.min.css";
import { THEME_TOKENS } from "@/lib/theme";
import { applyResponse, computeCurvePoints } from "./tuning";

/**
 * Response curve (UI plan P4.2 / §1.1): uPlot canvas, primary.
 *
 * - The curve is the sampled deadzone+curve mapping (input 0..1 ->
 *   output 0..1). Axes are hidden and the plot fills the container
 *   exactly, so the live-input dot (an HTML overlay positioned by %)
 *   lines up with the curve in both render paths.
 * - uPlot needs a real canvas 2D context. Where that is unavailable
 *   (jsdom tests, or a WebView2 failure) the constructor throws and the
 *   component falls back to an SVG polyline of the same points — the
 *   curve is never blank (C# blank-window lesson). `setData` failures
 *   fall back the same way.
 * - Chart colours come from the theme tokens in `src/lib/theme` (never a
 *   hard-coded hex in this file); the SVG path uses the CSS variable.
 * - Redraws happen only when the values change (R-P6): `setData` on
 *   deadzone/curve/live changes, nothing per frame.
 */

export interface ResponseCurveProps {
  /** Deadzone in percent (0..30). */
  deadzone: number;
  /** Response exponent (0.50..2.00, 1.00 = linear). */
  curve: number;
  /** Live input 0..1 from the snapshot feed, or null when unavailable. */
  liveInput: number | null;
  /** Accessible name for the chart region. */
  ariaLabel: string;
}

const CHART_HEIGHT = 220;

/** Accent stroke for the uPlot line, from the active theme tokens. */
function accentStroke(): string {
  const theme = document.documentElement.getAttribute("data-theme");
  if (theme === "dark" || theme === "high-contrast") {
    return THEME_TOKENS[theme]["--accent"];
  }
  return THEME_TOKENS.light["--accent"];
}

export function ResponseCurve({
  deadzone,
  curve,
  liveInput,
  ariaLabel,
}: ResponseCurveProps) {
  const points = useMemo(
    () => computeCurvePoints(deadzone, curve),
    [deadzone, curve],
  );
  const xs = useMemo(() => points.map((p) => p.x), [points]);
  const ys = useMemo(() => points.map((p) => p.y), [points]);
  const liveY =
    liveInput === null ? null : applyResponse(liveInput, deadzone, curve);

  const containerRef = useRef<HTMLDivElement>(null);
  const plotRef = useRef<uPlot | null>(null);
  const latestDataRef = useRef({ xs, ys });
  const [fallback, setFallback] = useState(false);
  const [width, setWidth] = useState(560);

  // Measure the container once + on resize (uPlot needs explicit pixels).
  useEffect(() => {
    const measure = () => {
      const measured = containerRef.current?.clientWidth;
      if (measured && measured > 0) setWidth(measured);
    };
    measure();
    window.addEventListener("resize", measure);
    return () => window.removeEventListener("resize", measure);
  }, []);

  // Create the uPlot instance (once per width); fall back to SVG on error.
  // uPlot is dynamically imported: its module scope touches matchMedia and
  // a bare canvas, so a static import would crash the whole screen in any
  // environment without them. A failed load falls back to the SVG curve.
  useEffect(() => {
    if (fallback) return;
    const el = containerRef.current;
    if (!el) return;
    let plot: uPlot | null = null;
    let cancelled = false;
    void (async () => {
      try {
        const mod = await import("uplot");
        const UPlot =
          (mod as unknown as { default?: typeof uPlot }).default ??
          (mod as unknown as typeof uPlot);
        if (cancelled) return;
        plot = new UPlot(
          {
            width,
            height: CHART_HEIGHT,
            cursor: { show: false },
            legend: { show: false },
            padding: [4, 4, 4, 4],
            scales: {
              x: { time: false, range: [0, 1] },
              y: { range: [0, 1] },
            },
            axes: [{ show: false }, { show: false }],
            series: [
              { label: "Input" },
              { label: "Output", stroke: accentStroke(), width: 2 },
            ],
          },
          [xs, ys],
          el,
        );
        plotRef.current = plot;
        // Values may have changed while the chunk loaded; push the latest.
        plot.setData([
          latestDataRef.current.xs,
          latestDataRef.current.ys,
        ]);
      } catch {
        if (!cancelled) {
          plotRef.current = null;
          setFallback(true);
        }
      }
    })();
    return () => {
      cancelled = true;
      plot?.destroy();
      plotRef.current = null;
    };
    // Values flow through the setData effect below, not through recreation.
  }, [fallback, width]);

  // Push new values into the live plot (value change only, R-P6).
  useEffect(() => {
    latestDataRef.current = { xs, ys };
    const plot = plotRef.current;
    if (!plot || fallback) return;
    try {
      plot.setData([xs, ys]);
    } catch {
      setFallback(true);
    }
  }, [xs, ys, fallback]);

  const svgPoints = points
    .map((p) => `${(p.x * 100).toFixed(2)},${((1 - p.y) * 100).toFixed(2)}`)
    .join(" ");

  return (
    <div className="space-y-1">
      <div
        role="img"
        aria-label={ariaLabel}
        className="relative h-[220px] w-full overflow-hidden rounded-xl border border-[var(--line)] bg-[var(--card)]"
      >
        {[25, 50, 75].map((p) => (
          <div
            key={`v${p}`}
            aria-hidden="true"
            className="absolute bottom-0 top-0 w-px bg-[var(--line)]"
            style={{ left: `${p}%` }}
          />
        ))}
        {[25, 50, 75].map((p) => (
          <div
            key={`h${p}`}
            aria-hidden="true"
            className="absolute left-0 right-0 h-px bg-[var(--line)]"
            style={{ top: `${p}%` }}
          />
        ))}
        <div ref={containerRef} className="absolute inset-0" aria-hidden="true" />
        {fallback && (
          <svg
            aria-hidden="true"
            className="absolute inset-0 h-full w-full"
            viewBox="0 0 100 100"
            preserveAspectRatio="none"
          >
            <polyline
              points={svgPoints}
              fill="none"
              strokeWidth={2}
              vectorEffect="non-scaling-stroke"
              style={{ stroke: "var(--accent)" }}
            />
          </svg>
        )}
        {liveInput !== null && liveY !== null && (
          <span
            aria-hidden="true"
            className="absolute h-3.5 w-3.5 -translate-x-1/2 translate-y-1/2 rounded-full border-2 border-[var(--card)] bg-[var(--accent)]"
            style={{
              left: `${liveInput * 100}%`,
              bottom: `${liveY * 100}%`,
            }}
          />
        )}
      </div>
      <div
        aria-hidden="true"
        className="flex justify-between px-1 text-xs text-[var(--muted)]"
      >
        <span>0%</span>
        <span>50%</span>
        <span>100%</span>
      </div>
    </div>
  );
}
