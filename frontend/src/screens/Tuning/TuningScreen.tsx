import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  getSettings,
  subscribeEngineSnapshot,
  type EngineSnapshot,
} from "@/lib/tauri";
import { Badge } from "@/components/ui/badge";
import { MODULES, parseModuleStates, type ModuleId } from "./modules";
import { TuningPanel } from "./TuningPanel";
import { cn } from "@/lib/utils";

/**
 * Tuning tab (UI plan P4.1): grid of 10 module cards, one panel at a time.
 *
 * - Each card shows the module name, icon, live On/Off state (read from
 *   the same settings store the Settings toggles write — see modules.ts)
 *   and a live-input mini value where the module has a snapshot channel.
 * - Tap a card → only that module's panel opens below the grid; tapping
 *   it again (or the panel's close button) closes it. An off module's
 *   card is dimmed and its panel opens read-only.
 * - There is NO toggle on any card or in any panel: module toggles live
 *   ONLY in Settings (plan §0 rule 2 + §P9.1).
 * - Module state is "Unknown" while the backend is not implemented —
 *   the cards and panels still work; nothing is faked.
 */

export function TuningScreen() {
  const { t } = useTranslation();
  const [selected, setSelected] = useState<ModuleId | null>(null);
  const [moduleStates, setModuleStates] = useState<
    Partial<Record<ModuleId, boolean>>
  >({});
  const [snapshot, setSnapshot] = useState<EngineSnapshot | null>(null);

  // Module on/off states (best effort — unknown when the backend is missing).
  useEffect(() => {
    let cancelled = false;
    void getSettings()
      .then((settings) => {
        if (!cancelled) setModuleStates(parseModuleStates(settings));
      })
      .catch(() => {
        // NOT_IMPLEMENTED / no backend yet: states stay "Unknown".
      });
    return () => {
      cancelled = true;
    };
  }, []);

  // One snapshot subscription for the cards' live mini values (4 Hz max).
  useEffect(() => {
    let done = false;
    let unlisten: (() => void) | undefined;
    let lastUpdate = 0;
    void subscribeEngineSnapshot((next) => {
      const now = Date.now();
      if (now - lastUpdate < 250) return;
      lastUpdate = now;
      if (!done) setSnapshot(next);
    })
      .then((stop) => {
        if (done) stop();
        else unlisten = stop;
      })
      .catch(() => {
        // No live feed: cards show "—" instead of a faked value.
      });
    return () => {
      done = true;
      unlisten?.();
    };
  }, []);

  const selectedModule =
    MODULES.find((m) => m.id === selected) ?? null;

  return (
    <section aria-label={t("tuning.title", { defaultValue: "Tuning" })}>
      <header className="mb-5">
        <h1 className="text-[26px] font-bold leading-tight">
          {t("tuning.title", { defaultValue: "Tuning" })}
        </h1>
        <p className="mt-1 max-w-2xl text-sm text-[var(--muted)]">
          {t("tuning.subtitle", {
            defaultValue:
              "Fine-tune each module's response. Select a module to open its tuning panel.",
          })}
        </p>
        <p className="mt-1 max-w-2xl text-sm text-[var(--muted)]">
          {t("tuning.profileOnlyNote", {
            defaultValue:
              "Tuning is saved to the active profile. The engine still runs its default plan, so tuning does not change the wheel output yet.",
          })}
        </p>
      </header>

      <div className="grid grid-cols-1 gap-4 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-5">
        {MODULES.map((module) => {
          const Icon = module.icon;
          const isSelected = selected === module.id;
          const state = moduleStates[module.id] ?? null;
          const label = t(module.labelKey, { defaultValue: module.id });
          const hint = t(module.hintKey, { defaultValue: "" });
          const stateText =
            state === true
              ? t("tuning.state.on", { defaultValue: "On" })
              : state === false
                ? t("tuning.state.off", { defaultValue: "Off" })
                : t("tuning.state.unknown", { defaultValue: "Unknown" });
          const liveValue =
            snapshot && module.liveValueOf
              ? module.liveValueOf(snapshot)
              : null;
          return (
            <button
              key={module.id}
              type="button"
              aria-pressed={isSelected}
              // Explicit name: JSX strips the whitespace between the
              // card's spans, so a name computed from content would
              // concatenate ("OnSteeringSteering wheel response").
              aria-label={`${label}: ${hint} (${stateText})`}
              onClick={() => setSelected(isSelected ? null : module.id)}
              className={cn(
                "rounded-2xl border bg-[var(--card)] p-5 text-left shadow-[var(--shadow)] transition-colors",
                isSelected
                  ? "border-[var(--accent)] ring-2 ring-[var(--accent)]"
                  : "border-[var(--line)] hover:border-[var(--accent)]",
                state === false && "opacity-60",
              )}
            >
              <span className="flex items-start justify-between gap-2">
                <span
                  aria-hidden="true"
                  className="flex h-10 w-10 items-center justify-center rounded-[12px] bg-[var(--accent-soft)] text-[var(--accent-deep)]"
                >
                  <Icon className="h-5 w-5" />
                </span>
                <Badge
                  variant={
                    state === true
                      ? "success"
                      : state === false
                        ? "outline"
                        : "secondary"
                  }
                >
                  {state === true
                    ? t("tuning.state.on", { defaultValue: "On" })
                    : state === false
                      ? t("tuning.state.off", { defaultValue: "Off" })
                      : t("tuning.state.unknown", {
                          defaultValue: "Unknown",
                        })}
                </Badge>
              </span>
              <span className="mt-3 block text-[15px] font-bold leading-tight">
                {label}
              </span>
              <span className="mt-1 block text-[13px] text-[var(--muted)]">
                {hint}
              </span>
              {module.liveValueOf && (
                <span className="mt-2 block text-xs font-semibold tabular-nums text-[var(--accent-deep)]">
                  {liveValue === null
                    ? "—"
                    : `${t("tuning.liveInput", { defaultValue: "Live input" })}: ${Math.round(liveValue * 100)}%`}
                </span>
              )}
            </button>
          );
        })}
      </div>

      {selectedModule && (
        <div className="mt-5">
          <TuningPanel
            module={selectedModule}
            enabled={moduleStates[selectedModule.id] ?? null}
            onClose={() => setSelected(null)}
          />
        </div>
      )}
    </section>
  );
}
