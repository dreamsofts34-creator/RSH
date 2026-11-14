import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Check, Lock, Wrench } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import i18n from "@/lib/i18n";
import { cn } from "@/lib/utils";
import { ConnectStep } from "./ConnectStep";
import { PairingStep } from "./PairingStep";
import { PasswordStep } from "./PasswordStep";
import { PinsStep } from "./PinsStep";
import { CalibrateStep } from "./CalibrateStep";
import { FirmwareStep } from "./FirmwareStep";
import { FinishStep } from "./FinishStep";
import enStrings from "./strings.en.json";
import urStrings from "./strings.ur.json";
import hiStrings from "./strings.hi.json";

/**
 * Setup wizard screen (UI plan P5: Connect → Pairing → Change password →
 * Pins → Calibrate → Firmware → Finish).
 *
 * The plan's wizard is 3 steps (Connect → Pins → Calibrate, P5.1–P5.3);
 * this wizard keeps that order and adds the steps the Phase E3 scope
 * requires around it: pairing + change-password (P5.2, Security plan)
 * after Connect, and the firmware upload gate (SEC-R2-01) before Finish.
 *
 * Strings live in `strings.{en,ur,hi}.json` next to this file (the
 * coordinator merges them into `src/locales/` at the Phase E gate).
 * They are also registered here with `addResourceBundle(..., overwrite:
 * false)` so the screen renders translated text today, before that
 * merge — the merged bundles win once they exist.
 */
i18n.addResourceBundle("en", "translation", enStrings, true, false);
i18n.addResourceBundle("ur", "translation", urStrings, true, false);
i18n.addResourceBundle("hi", "translation", hiStrings, true, false);

export const STEP_IDS = [
  "connect",
  "pairing",
  "password",
  "pins",
  "calibrate",
  "firmware",
  "finish",
] as const;

type StepId = (typeof STEP_IDS)[number];

const STEP_COMPONENTS: Record<
  Exclude<StepId, "finish">,
  (props: { onSuccessChange: (ok: boolean) => void }) => React.JSX.Element
> = {
  connect: ConnectStep,
  pairing: PairingStep,
  password: PasswordStep,
  pins: PinsStep,
  calibrate: CalibrateStep,
  firmware: FirmwareStep,
};

const STORAGE_COMPLETED = "rsh.setup.completed";
const STORAGE_CURRENT = "rsh.setup.currentStep";
const STORAGE_DONE = "rsh.setup.done";
const STORAGE_VISITED = "rsh.setup.visited";
const STORAGE_SUCCESS = "rsh.setup.success";

function readStoredCompleted(): string[] {
  try {
    const raw = window.localStorage.getItem(STORAGE_COMPLETED);
    if (raw === null) return [];
    const parsed: unknown = JSON.parse(raw);
    if (!Array.isArray(parsed)) return [];
    return parsed.filter((id): id is StepId =>
      STEP_IDS.includes(id as StepId),
    );
  } catch {
    return [];
  }
}

function readStoredCurrent(): number {
  try {
    const raw = window.localStorage.getItem(STORAGE_CURRENT);
    const index = raw === null ? 0 : Number(raw);
    return Number.isInteger(index) && index >= 0 && index < STEP_IDS.length
      ? index
      : 0;
  } catch {
    return 0;
  }
}

function readStoredDone(): boolean {
  try {
    return window.localStorage.getItem(STORAGE_DONE) === "true";
  } catch {
    return false;
  }
}

function readStoredStringList(key: string): string[] {
  try {
    const raw = window.localStorage.getItem(key);
    if (raw === null) return [];
    const parsed: unknown = JSON.parse(raw);
    if (!Array.isArray(parsed)) return [];
    return parsed.filter((id): id is StepId =>
      STEP_IDS.includes(id as StepId),
    );
  } catch {
    return [];
  }
}

function readStoredSuccess(): Partial<Record<StepId, boolean>> {
  try {
    const raw = window.localStorage.getItem(STORAGE_SUCCESS);
    if (raw === null) return {};
    const parsed: unknown = JSON.parse(raw);
    if (parsed === null || typeof parsed !== "object") return {};
    const out: Partial<Record<StepId, boolean>> = {};
    for (const [key, value] of Object.entries(
      parsed as Record<string, unknown>,
    )) {
      if (STEP_IDS.includes(key as StepId) && typeof value === "boolean") {
        out[key as StepId] = value;
      }
    }
    return out;
  } catch {
    return {};
  }
}

export function SetupScreen() {
  const { t } = useTranslation();
  const [current, setCurrent] = useState(readStoredCurrent);
  const [completed, setCompleted] = useState<string[]>(readStoredCompleted);
  const [done, setDone] = useState(readStoredDone);
  /**
   * H5: "visited" (the user has opened the step) is tracked separately
   * from "completed" (the step's success condition held when Next was
   * pressed). A visited step can always be revisited; only completed
   * steps unlock the NEXT step.
   */
  const [visited, setVisited] = useState<string[]>(() =>
    readStoredStringList(STORAGE_VISITED),
  );
  /** Per-step success as reported by the steps themselves (H5). */
  const [stepSuccess, setStepSuccess] = useState<
    Partial<Record<StepId, boolean>>
  >(readStoredSuccess);
  const [nextBlocked, setNextBlocked] = useState(false);

  // Wizard progress persists per step (completed steps stay checkmarked
  // across reloads; the current step is restored too).
  useEffect(() => {
    try {
      window.localStorage.setItem(STORAGE_COMPLETED, JSON.stringify(completed));
      window.localStorage.setItem(STORAGE_CURRENT, String(current));
      window.localStorage.setItem(STORAGE_DONE, String(done));
      window.localStorage.setItem(STORAGE_VISITED, JSON.stringify(visited));
      window.localStorage.setItem(STORAGE_SUCCESS, JSON.stringify(stepSuccess));
    } catch {
      // Storage unavailable (private mode, etc.): the wizard still works
      // for this session, progress just does not persist.
    }
  }, [completed, current, done, visited, stepSuccess]);

  // The current step counts as visited as soon as it renders.
  useEffect(() => {
    const id = STEP_IDS[current];
    setVisited((prev) => (prev.includes(id) ? prev : [...prev, id]));
  }, [current]);

  const completedSet = new Set(completed);
  const visitedSet = new Set(visited);
  /**
   * H5/L8: furthest reachable step = one past the highest COMPLETED
   * index (not `completed.length` — a non-contiguous completed list
   * used to unlock steps that were never earned). The current step is
   * always reachable.
   */
  const maxCompletedIndex = Math.max(
    -1,
    ...completed.map((id) => STEP_IDS.indexOf(id as StepId)),
  );
  const furthest = Math.min(
    STEP_IDS.length - 1,
    Math.max(current, maxCompletedIndex + 1),
  );
  const currentId = STEP_IDS[current];
  // The Finish step needs the wizard's progress, so the shell renders it
  // directly instead of through the step table.
  const CurrentStep =
    currentId === "finish" ? null : STEP_COMPONENTS[currentId];

  const handleStepSuccess = useCallback((id: StepId, ok: boolean) => {
    setStepSuccess((prev) => (prev[id] === ok ? prev : { ...prev, [id]: ok }));
  }, []);

  function goTo(index: number) {
    // Future steps stay locked until the user reaches them in order —
    // except steps already visited, which can always be revisited.
    if (index < 0 || index >= STEP_IDS.length) return;
    if (index > furthest && !visitedSet.has(STEP_IDS[index])) return;
    setCurrent(index);
    setNextBlocked(false);
  }

  function handleNext() {
    // H5: Next is gated on the current step's SUCCESS state — merely
    // visiting a step never completes it. A step completed earlier
    // stays complete on revisit.
    const succeeded =
      stepSuccess[currentId] === true || completedSet.has(currentId);
    if (!succeeded) {
      setNextBlocked(true);
      return;
    }
    setNextBlocked(false);
    if (!completedSet.has(currentId)) {
      setCompleted((prev) => [...prev, currentId]);
    }
    setCurrent((prev) => Math.min(prev + 1, STEP_IDS.length - 1));
  }

  function handleBack() {
    setCurrent((prev) => Math.max(prev - 1, 0));
    setNextBlocked(false);
  }

  function handleStartOver() {
    setCompleted([]);
    setCurrent(0);
    setDone(false);
    setVisited([]);
    setStepSuccess({});
    setNextBlocked(false);
  }

  function handleFinish() {
    if (!completedSet.has("finish")) {
      setCompleted((prev) => [...prev, "finish"]);
    }
    setDone(true);
  }

  return (
    <div className="mx-auto w-full max-w-3xl space-y-4 p-4">
      <Card>
        <CardHeader>
          <CardTitle>{t("setup.title")}</CardTitle>
          <p className="text-sm text-[var(--muted)]">{t("setup.subtitle")}</p>
        </CardHeader>
        <CardContent className="space-y-5">
          <div className="space-y-2">
            <ol className="flex flex-wrap gap-2" aria-label={t("setup.title")}>
              {STEP_IDS.map((id, index) => {
                const isDone = completedSet.has(id);
                const isCurrent = index === current;
                const isLocked =
                  index > furthest && !visitedSet.has(id);
                return (
                  <li key={id}>
                    <button
                      type="button"
                      onClick={() => goTo(index)}
                      disabled={isLocked}
                      aria-current={isCurrent ? "step" : undefined}
                      className={cn(
                        "flex items-center gap-1.5 rounded-full border px-3 py-1.5 text-sm font-semibold",
                        isCurrent &&
                          "border-[var(--accent)] bg-[var(--accent-soft)] text-[var(--accent-deep)]",
                        !isCurrent &&
                          isDone &&
                          "border-[var(--line)] bg-[var(--card)] text-[var(--ink)]",
                        !isCurrent &&
                          !isDone &&
                          !isLocked &&
                          "border-[var(--line)] bg-[var(--card)] text-[var(--muted)]",
                        isLocked && "cursor-not-allowed opacity-45",
                      )}
                    >
                      {isDone ? (
                        <Check className="size-4" aria-label="completed" />
                      ) : isLocked ? (
                        <Lock className="size-4" aria-label="locked" />
                      ) : (
                        <span aria-hidden="true">{index + 1}</span>
                      )}
                      {t(`setup.${id}.title`)}
                    </button>
                  </li>
                );
              })}
            </ol>
            <p className="text-sm text-[var(--muted)]">
              {t("setup.stepOf", { current: current + 1, total: STEP_IDS.length })}
            </p>
          </div>

          <section aria-label={t(`setup.${currentId}.title`)}>
            {currentId === "finish" ? (
              <FinishStep
                completed={completed}
                stepIds={STEP_IDS}
                done={done}
                onFinish={handleFinish}
              />
            ) : (
              CurrentStep !== null && (
                <CurrentStep
                  onSuccessChange={(ok) => handleStepSuccess(currentId, ok)}
                />
              )
            )}
          </section>

          <div className="flex flex-wrap items-center gap-2 border-t border-[var(--line)] pt-4">
            <Button
              type="button"
              variant="outline"
              onClick={handleBack}
              disabled={current === 0}
            >
              {t("setup.back")}
            </Button>
            {current < STEP_IDS.length - 1 && (
              <Button type="button" onClick={handleNext}>
                {t("setup.next")}
              </Button>
            )}
            {nextBlocked ? (
              <p role="alert" className="text-sm text-[var(--error-text)]">
                {t("setup.nextBlocked")}
              </p>
            ) : null}
            <span className="flex-1" />
            <Button type="button" variant="ghost" onClick={handleStartOver}>
              {t("setup.startOver")}
            </Button>
          </div>
        </CardContent>
      </Card>
    </div>
  );
}

export const path = "setup";
export const label = "Setup";
export const icon = Wrench;
export const component = SetupScreen;
