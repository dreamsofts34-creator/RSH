import type { TuningSettings } from "@/lib/tauri";

/**
 * Tuning value model (UI plan P4.2).
 *
 * The backend contract is `TuningSettings` (src/lib/tauri.ts) — one
 * uniform record per module: deadzone, curve, min/max endpoints in mV,
 * smoothing, plus the optional module-specific extensions (pedal bite
 * point, shifter gate type, horn button assignment, LED
 * brightness/pattern). The base five sliders render for every module;
 * the extension fields are carried through `defaultTuning` /
 * `sameTuning` / `normalizeTuning` untouched (validated, never
 * invented) so a save never drops them — and the panel renders
 * controls for them on the modules that own them (C-TUNE-1).
 *
 * Chosen values (documented, not backend-verified — the backend commands
 * are NOT_IMPLEMENTED in src-tauri today):
 * - Deadzone 0–30 % (displayed as %; stored as the same number).
 * - Curve 0.50–2.00, step 0.05; 1.00 is linear.
 * - Endpoints 0–3300 mV, step 10 mV; span must stay >= 20 % of full scale
 *   (660 mV, Software plan P9 contract) — enforced by `clampEndpoints`.
 * - Smoothing 0–100 % (One Euro factor, displayed as %).
 * - Defaults: deadzone 0, curve 1.00, min 0 mV, max 3300 mV, smoothing 0.
 * - Extensions: bitePoint 0..1, hornButton 1..32, ledBrightness 0..100,
 *   shifterGate "sequential" | "h-pattern", ledPattern one of
 *   "off" | "solid" | "blink" | "chase" | "rainbow".
 */

/** Full-scale ADC range in mV (ESP32 ADC, 0..3300 mV). */
export const FULL_SCALE_MV = 3300;

/** Endpoints must span at least 20 % of full scale (660 mV). */
export const MIN_SPAN_MV = 660;

/** Slider ranges (min / max / step) per tuning field. */
export const TUNING_RANGES = {
  deadzone: { min: 0, max: 30, step: 1 },
  curve: { min: 0.5, max: 2, step: 0.05 },
  minMv: { min: 0, max: FULL_SCALE_MV, step: 10 },
  maxMv: { min: 0, max: FULL_SCALE_MV, step: 10 },
  smoothing: { min: 0, max: 100, step: 1 },
} as const;

export type TuningField = keyof Omit<TuningSettings, "module">;

/** Factory defaults for a module (the Reset-to-defaults target). */
export function defaultTuning(module: string): TuningSettings {
  return {
    module,
    deadzone: 0,
    curve: 1,
    minMv: 0,
    maxMv: FULL_SCALE_MV,
    smoothing: 0,
  };
}

/** Field-by-field equality (draft vs last-saved), including extensions. */
export function sameTuning(a: TuningSettings, b: TuningSettings): boolean {
  return (
    a.module === b.module &&
    a.deadzone === b.deadzone &&
    a.curve === b.curve &&
    a.minMv === b.minMv &&
    a.maxMv === b.maxMv &&
    a.smoothing === b.smoothing &&
    a.bitePoint === b.bitePoint &&
    a.shifterGate === b.shifterGate &&
    a.hornButton === b.hornButton &&
    a.ledBrightness === b.ledBrightness &&
    a.ledPattern === b.ledPattern
  );
}

export function clampNumber(value: number, min: number, max: number): number {
  if (!Number.isFinite(value)) return min;
  return Math.min(max, Math.max(min, value));
}

/**
 * Enforce the endpoint span guard. `changed` says which slider the user
 * moved: the other endpoint stays put and the moved one is clamped back.
 * Returns the adjusted pair and whether anything was clamped.
 */
export function clampEndpoints(
  minMv: number,
  maxMv: number,
  changed: "min" | "max",
): { minMv: number; maxMv: number; clamped: boolean } {
  let min = clampNumber(minMv, 0, FULL_SCALE_MV);
  let max = clampNumber(maxMv, 0, FULL_SCALE_MV);
  let clamped = min !== minMv || max !== maxMv;
  if (max - min < MIN_SPAN_MV) {
    clamped = true;
    if (changed === "min") {
      min = max - MIN_SPAN_MV;
      if (min < 0) {
        min = 0;
        max = MIN_SPAN_MV;
      }
    } else {
      max = min + MIN_SPAN_MV;
      if (max > FULL_SCALE_MV) {
        max = FULL_SCALE_MV;
        min = FULL_SCALE_MV - MIN_SPAN_MV;
      }
    }
  }
  return { minMv: min, maxMv: max, clamped };
}

/**
 * Defensive normalisation of a backend record: non-finite or missing
 * numbers fall back to defaults, every field is clamped into range, and
 * the endpoint span guard is applied (preferring to lower the minimum).
 * Extension fields pass through only when they are valid values for
 * their module; anything else is dropped (never invented, never kept
 * corrupt).
 */
export function normalizeTuning(input: TuningSettings): TuningSettings {
  const fallback = defaultTuning(
    typeof input?.module === "string" ? input.module : "",
  );
  const finiteOr = (value: unknown, fallbackValue: number): number =>
    typeof value === "number" && Number.isFinite(value)
      ? value
      : fallbackValue;
  const endpoints = clampEndpoints(
    finiteOr(input?.minMv, fallback.minMv),
    finiteOr(input?.maxMv, fallback.maxMv),
    "min",
  );
  const bitePoint =
    typeof input?.bitePoint === "number" && Number.isFinite(input.bitePoint)
      ? clampNumber(input.bitePoint, 0, 1)
      : undefined;
  const shifterGate =
    input?.shifterGate === "sequential" || input?.shifterGate === "h-pattern"
      ? input.shifterGate
      : undefined;
  const hornButton =
    typeof input?.hornButton === "number" &&
    Number.isInteger(input.hornButton) &&
    input.hornButton >= 1 &&
    input.hornButton <= 32
      ? input.hornButton
      : undefined;
  const ledBrightness =
    typeof input?.ledBrightness === "number" &&
    Number.isFinite(input.ledBrightness)
      ? Math.round(clampNumber(input.ledBrightness, 0, 100))
      : undefined;
  const ledPattern =
    input?.ledPattern === "off" ||
    input?.ledPattern === "solid" ||
    input?.ledPattern === "blink" ||
    input?.ledPattern === "chase" ||
    input?.ledPattern === "rainbow"
      ? input.ledPattern
      : undefined;
  return {
    module: fallback.module,
    deadzone: clampNumber(
      finiteOr(input?.deadzone, fallback.deadzone),
      TUNING_RANGES.deadzone.min,
      TUNING_RANGES.deadzone.max,
    ),
    curve: clampNumber(
      finiteOr(input?.curve, fallback.curve),
      TUNING_RANGES.curve.min,
      TUNING_RANGES.curve.max,
    ),
    minMv: endpoints.minMv,
    maxMv: endpoints.maxMv,
    smoothing: clampNumber(
      finiteOr(input?.smoothing, fallback.smoothing),
      TUNING_RANGES.smoothing.min,
      TUNING_RANGES.smoothing.max,
    ),
    bitePoint,
    shifterGate,
    hornButton,
    ledBrightness,
    ledPattern,
  };
}

/**
 * Response mapping: raw input 0..1 -> shaped output 0..1.
 * Inputs at or below the deadzone map to 0; the rest is normalised and
 * raised to the curve exponent (1.00 = linear).
 */
export function applyResponse(
  input: number,
  deadzonePct: number,
  curve: number,
): number {
  const deadzone = clampNumber(deadzonePct, 0, 100) / 100;
  if (input <= deadzone || deadzone >= 1) return 0;
  const normalised = (input - deadzone) / (1 - deadzone);
  return Math.pow(clampNumber(normalised, 0, 1), curve);
}

export interface CurvePoint {
  x: number;
  y: number;
}

/** Sampled response curve (61 points by default) for the chart. */
export function computeCurvePoints(
  deadzonePct: number,
  curve: number,
  steps = 60,
): CurvePoint[] {
  const points: CurvePoint[] = [];
  for (let i = 0; i <= steps; i++) {
    const x = i / steps;
    points.push({ x, y: applyResponse(x, deadzonePct, curve) });
  }
  return points;
}
