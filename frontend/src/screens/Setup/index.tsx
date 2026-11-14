import { useEffect, useState } from "react";
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

const STEP_COMPONENTS: Record<Exclude<StepId, "finish">, () => React.JSX.Element> = {
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

export function SetupScreen() {
  const { t } = useTranslation();
  const [current, setCurrent] = useState(readStoredCurrent);
  const [completed, setCompleted] = useState<string[]>(readStoredCompleted);
  const [done, setDone] = useState(readStoredDone);

  // Wizard progress persists per step (completed steps stay checkmarked
  // across reloads; the current step is restored too).
  useEffect(() => {
    try {
      window.localStorage.setItem(STORAGE_COMPLETED, JSON.stringify(completed));
      window.localStorage.setItem(STORAGE_CURRENT, String(current));
      window.localStorage.setItem(STORAGE_DONE, String(done));
    } catch {
      // Storage unavailable (private mode, etc.): the wizard still works
      // for this session, progress just does not persist.
    }
  }, [completed, current, done]);

  const completedSet = new Set(completed);
  // Furthest step the user has reached: completed steps + the current one.
  const furthest = Math.max(current, completed.length);
  const currentId = STEP_IDS[current];
  // The Finish step needs the wizard's progress, so the shell renders it
  // directly instead of through the step table.
  const CurrentStep =
    currentId === "finish" ? null : STEP_COMPONENTS[currentId];

  function goTo(index: number) {
    // Future steps stay locked until the user reaches them in order.
    if (index < 0 || index > furthest || index >= STEP_IDS.length) return;
    setCurrent(index);
  }

  function handleNext() {
    if (!completedSet.has(currentId)) {
      setCompleted((prev) => [...prev, currentId]);
    }
    setCurrent((prev) => Math.min(prev + 1, STEP_IDS.length - 1));
  }

  function handleBack() {
    setCurrent((prev) => Math.max(prev - 1, 0));
  }

  function handleStartOver() {
    setCompleted([]);
    setCurrent(0);
    setDone(false);
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
                const isLocked = index > furthest;
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
              CurrentStep !== null && <CurrentStep />
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
