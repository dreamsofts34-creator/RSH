import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { RotateCcw, X } from "lucide-react";
import {
  getTuning,
  setTuning,
  subscribeEngineSnapshot,
  TauriCommandError,
  type TuningSettings,
} from "@/lib/tauri";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader } from "@/components/ui/card";
import type { ModuleDef } from "./modules";
import { FieldSlider } from "./FieldSlider";
import { ResponseCurve } from "./ResponseCurve";
import {
  clampEndpoints,
  defaultTuning,
  normalizeTuning,
  sameTuning,
  TUNING_RANGES,
  type TuningField,
} from "./tuning";

/**
 * Per-module tuning panel (UI plan P4.2). Exactly one panel is open at a
 * time — the screen owns that state; this panel only tunes its module.
 *
 * - Load: `get_tuning` on open. While loading, a loading state; if the
 *   backend has not implemented the command, an honest NOT_IMPLEMENTED
 *   state with Retry — never a dead slider. Other failures show the
 *   backend message, also with Retry.
 * - Edit: sliders update the draft immediately (the curve redraws live)
 *   and autosave debounced 400 ms (R-P4) via `set_tuning`. Reset sets
 *   the draft to defaults; the same autosave persists it.
 * - Range guard: endpoints must span >= 660 mV; a violating move is
 *   clamped and a message says so (silent clamp is banned by P4.2).
 * - Live input: the 30 Hz snapshot feed (throttled to 10 Hz here, R-P6)
 *   drives the marker dot on the curve, for modules that have a
 *   snapshot channel. A failed subscription degrades to "no live input".
 * - No enable toggle here: module on/off lives ONLY in Settings
 *   (plan §0 rule 2 + §P9.1). `enabled === false` renders the panel
 *   read-only with a notice; `enabled === null` means unknown (backend
 *   not implemented yet) and the panel works normally.
 */

/** Debounce for autosave (R-P4: 400 ms on text/config input). */
const SAVE_DEBOUNCE_MS = 400;

type SaveState = "idle" | "saving" | "saved" | "error";

export interface TuningPanelProps {
  module: ModuleDef;
  /** true = on, false = off, null = unknown (no backend state yet). */
  enabled: boolean | null;
  onClose: () => void;
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error ?? "unknown");
}

export function TuningPanel({ module, enabled, onClose }: TuningPanelProps) {
  const { t } = useTranslation();
  const [status, setStatus] = useState<"loading" | "ready" | "error">(
    "loading",
  );
  const [loadError, setLoadError] = useState<{
    notImplemented: boolean;
    message: string;
  } | null>(null);
  const [loadNonce, setLoadNonce] = useState(0);
  const [draft, setDraft] = useState<TuningSettings | null>(null);
  const [saveState, setSaveState] = useState<SaveState>("idle");
  const [saveError, setSaveError] = useState<string | null>(null);
  const [spanGuard, setSpanGuard] = useState(false);
  const [liveInput, setLiveInput] = useState<number | null>(null);

  const savedRef = useRef<TuningSettings | null>(null);
  const saveTimerRef = useRef<number | undefined>(undefined);

  const label = t(module.labelKey, { defaultValue: module.id });
  const controlsDisabled = enabled === false || status !== "ready";

  // Load current tuning whenever the module (or a retry) changes.
  useEffect(() => {
    let cancelled = false;
    setStatus("loading");
    setLoadError(null);
    setDraft(null);
    setSaveState("idle");
    setSaveError(null);
    setSpanGuard(false);
    void getTuning({ module: module.id })
      .then((tuning) => {
        if (cancelled) return;
        const normalised = normalizeTuning(tuning);
        savedRef.current = normalised;
        setDraft(normalised);
        setStatus("ready");
      })
      .catch((error: unknown) => {
        if (cancelled) return;
        setLoadError({
          notImplemented:
            error instanceof TauriCommandError && error.notImplemented,
          message: errorMessage(error),
        });
        setStatus("error");
      });
    return () => {
      cancelled = true;
    };
  }, [module, loadNonce]);

  // Persist one draft through set_tuning.
  const saveNow = useCallback(async (values: TuningSettings) => {
    window.clearTimeout(saveTimerRef.current);
    setSaveState("saving");
    setSaveError(null);
    try {
      const saved = await setTuning(values);
      savedRef.current = saved;
      setDraft(saved);
      setSaveState("saved");
    } catch (error) {
      setSaveState("error");
      setSaveError(errorMessage(error));
    }
  }, []);

  // Debounced autosave: draft edits persist 400 ms after the last change.
  useEffect(() => {
    if (!draft || !savedRef.current) return;
    if (sameTuning(draft, savedRef.current)) return;
    setSaveState("saving");
    saveTimerRef.current = window.setTimeout(() => {
      // The reset path may already have saved this exact draft.
      if (sameTuning(draft, savedRef.current ?? draft)) return;
      void saveNow(draft);
    }, SAVE_DEBOUNCE_MS);
    return () => window.clearTimeout(saveTimerRef.current);
  }, [draft, saveNow]);

  // Live input marker (snapshot feed, throttled to 10 Hz, R-P6).
  useEffect(() => {
    if (!module.liveValueOf) return;
    const liveValueOf = module.liveValueOf;
    let done = false;
    let unlisten: (() => void) | undefined;
    let lastUpdate = 0;
    void subscribeEngineSnapshot((snapshot) => {
      const now = Date.now();
      if (now - lastUpdate < 100) return;
      lastUpdate = now;
      if (done) return;
      setLiveInput(liveValueOf(snapshot));
    })
      .then((stop) => {
        if (done) stop();
        else unlisten = stop;
      })
      .catch(() => {
        // No live feed (e.g. outside Tauri): the curve still renders.
      });
    return () => {
      done = true;
      unlisten?.();
    };
  }, [module]);

  function updateField(field: TuningField, value: number) {
    if (!draft) return;
    let next: TuningSettings = { ...draft, [field]: value };
    let clamped = false;
    if (field === "minMv" || field === "maxMv") {
      const fixed = clampEndpoints(next.minMv, next.maxMv, field === "minMv" ? "min" : "max");
      next = { ...next, minMv: fixed.minMv, maxMv: fixed.maxMv };
      clamped = fixed.clamped;
    }
    setSpanGuard(clamped);
    setDraft(next);
  }

  function handleReset() {
    setSpanGuard(false);
    setDraft(defaultTuning(module.id));
  }

  const stateBadge =
    enabled === true
      ? { variant: "success" as const, text: t("tuning.state.on", { defaultValue: "On" }) }
      : enabled === false
        ? { variant: "outline" as const, text: t("tuning.state.off", { defaultValue: "Off" }) }
        : { variant: "secondary" as const, text: t("tuning.state.unknown", { defaultValue: "Unknown" }) };

  return (
    <Card aria-label={`${label} tuning`}>
      <CardHeader className="flex flex-row items-start justify-between gap-4 space-y-0">
        <div className="space-y-1">
          <h2 className="text-lg font-bold leading-tight">
            {label}
          </h2>
          <div className="flex items-center gap-2">
            <Badge variant={stateBadge.variant}>{stateBadge.text}</Badge>
            <span className="text-xs text-[var(--muted)]">
              {t("tuning.manageInSettings", {
                defaultValue: "Module on/off is managed in Settings.",
              })}
            </span>
          </div>
        </div>
        <Button
          type="button"
          variant="ghost"
          size="icon"
          aria-label={t("tuning.closePanel", { defaultValue: "Close panel" })}
          onClick={onClose}
        >
          <X />
        </Button>
      </CardHeader>

      <CardContent className="space-y-4">
        {status === "loading" && (
          <p role="status">
            {t("tuning.loading", { defaultValue: "Loading…" })}
          </p>
        )}

        {status === "error" && loadError && (
          <div role="alert" className="space-y-3">
            <p className="font-semibold text-[var(--error-text)]">
              {loadError.notImplemented
                ? t("tuning.notImplemented", {
                    defaultValue: "Tuning is not available yet.",
                  })
                : t("tuning.loadError", {
                    defaultValue: "Could not load tuning.",
                  })}
            </p>
            <p className="text-sm text-[var(--muted)]">
              {loadError.notImplemented
                ? t("tuning.notImplementedDetail", {
                    defaultValue:
                      "The backend command get_tuning is not implemented yet.",
                  })
                : loadError.message}
            </p>
            <Button
              type="button"
              variant="outline"
              onClick={() => setLoadNonce((n) => n + 1)}
            >
              {t("tuning.retry", { defaultValue: "Retry" })}
            </Button>
          </div>
        )}

        {status === "ready" && draft && (
          <>
            {enabled === false && (
              <p
                role="note"
                className="rounded-lg bg-[var(--banner)] px-3 py-2 text-sm text-[var(--ink)]"
              >
                {t("tuning.moduleOff", {
                  defaultValue:
                    "This module is off. Turn it on in Settings to tune it.",
                })}
              </p>
            )}

            <div>
              <h3 className="mb-2 text-sm font-semibold">
                {t("tuning.curveTitle", {
                  defaultValue: "Response curve",
                })}
              </h3>
              <ResponseCurve
                deadzone={draft.deadzone}
                curve={draft.curve}
                liveInput={liveInput}
                ariaLabel={t("tuning.curveTitle", {
                  defaultValue: "Response curve",
                })}
              />
              <p className="mt-2 text-sm text-[var(--muted)]">
                {liveInput === null
                  ? t("tuning.noLiveInput", {
                      defaultValue: "No live input for this module.",
                    })
                  : `${t("tuning.liveInput", { defaultValue: "Live input" })}: ${Math.round(liveInput * 100)}%`}
              </p>
            </div>

            <div className="grid gap-5 sm:grid-cols-2">
              <FieldSlider
                label={t("tuning.deadzone", { defaultValue: "Deadzone" })}
                value={draft.deadzone}
                min={TUNING_RANGES.deadzone.min}
                max={TUNING_RANGES.deadzone.max}
                step={TUNING_RANGES.deadzone.step}
                disabled={controlsDisabled}
                valueText={`${draft.deadzone}%`}
                onChange={(value) => updateField("deadzone", value)}
              />
              <FieldSlider
                label={t("tuning.curve", { defaultValue: "Curve" })}
                value={draft.curve}
                min={TUNING_RANGES.curve.min}
                max={TUNING_RANGES.curve.max}
                step={TUNING_RANGES.curve.step}
                disabled={controlsDisabled}
                valueText={draft.curve.toFixed(2)}
                onChange={(value) => updateField("curve", value)}
              />
              <FieldSlider
                label={t("tuning.minMv", { defaultValue: "Minimum (mV)" })}
                value={draft.minMv}
                min={TUNING_RANGES.minMv.min}
                max={TUNING_RANGES.minMv.max}
                step={TUNING_RANGES.minMv.step}
                disabled={controlsDisabled}
                valueText={`${draft.minMv} mV`}
                onChange={(value) => updateField("minMv", value)}
              />
              <FieldSlider
                label={t("tuning.maxMv", { defaultValue: "Maximum (mV)" })}
                value={draft.maxMv}
                min={TUNING_RANGES.maxMv.min}
                max={TUNING_RANGES.maxMv.max}
                step={TUNING_RANGES.maxMv.step}
                disabled={controlsDisabled}
                valueText={`${draft.maxMv} mV`}
                onChange={(value) => updateField("maxMv", value)}
              />
              <FieldSlider
                label={t("tuning.smoothing", { defaultValue: "Smoothing" })}
                value={draft.smoothing}
                min={TUNING_RANGES.smoothing.min}
                max={TUNING_RANGES.smoothing.max}
                step={TUNING_RANGES.smoothing.step}
                disabled={controlsDisabled}
                valueText={`${draft.smoothing}%`}
                onChange={(value) => updateField("smoothing", value)}
              />
            </div>

            {spanGuard && (
              <p role="status" className="text-sm text-[var(--warning-text)]">
                {t("tuning.spanGuard", {
                  defaultValue:
                    "Endpoints must span at least 660 mV. The value was clamped.",
                })}
              </p>
            )}

            <div className="flex flex-wrap items-center gap-3">
              <Button
                type="button"
                variant="outline"
                disabled={controlsDisabled}
                onClick={handleReset}
              >
                <RotateCcw />
                {t("tuning.reset", { defaultValue: "Reset to defaults" })}
              </Button>
              {saveState === "saving" && (
                <span role="status" className="text-sm text-[var(--muted)]">
                  {t("tuning.saving", { defaultValue: "Saving…" })}
                </span>
              )}
              {saveState === "saved" && (
                <span role="status" className="text-sm text-[var(--success-text)]">
                  {t("tuning.saved", { defaultValue: "Saved" })}
                </span>
              )}
              {saveState === "error" && saveError && (
                <span role="alert" className="text-sm text-[var(--error-text)]">
                  {t("tuning.saveError", {
                    defaultValue: "Could not save tuning.",
                  })}{" "}
                  {saveError}
                </span>
              )}
            </div>
          </>
        )}
      </CardContent>
    </Card>
  );
}
