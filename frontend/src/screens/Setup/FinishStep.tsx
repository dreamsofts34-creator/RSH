import { useTranslation } from "react-i18next";
import { Check, Circle } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Notice, StepIntro } from "./components";

/**
 * Step 7 — Finish.
 *
 * Summarises which wizard steps are complete (from the persisted
 * progress) and marks the wizard done. Nothing here calls the backend:
 * finishing is a local acknowledgement that the wheel is set up.
 */

export interface FinishStepProps {
  /** Step ids the wizard recorded as complete, in step order. */
  completed: string[];
  /** All wizard step ids, in order, for the summary list. */
  stepIds: readonly string[];
  /** Whether the wizard has been marked done. */
  done: boolean;
  /** Mark the wizard done. */
  onFinish: () => void;
}

export function FinishStep({ completed, stepIds, done, onFinish }: FinishStepProps) {
  const { t } = useTranslation();
  const doneSet = new Set(completed);

  return (
    <div className="space-y-4">
      <StepIntro
        title={t("setup.finish.title")}
        description={t("setup.finish.description")}
      />

      {completed.length === 0 ? (
        <Notice tone="info">{t("setup.finish.noneYet")}</Notice>
      ) : (
        <div className="space-y-2">
          <p className="text-sm font-semibold text-[var(--ink)]">
            {t("setup.finish.completedSteps")}
          </p>
          <ul className="space-y-1">
            {stepIds.map((id) => (
              <li key={id} className="flex items-center gap-2 text-sm text-[var(--ink)]">
                {doneSet.has(id) ? (
                  <Check className="size-4 text-emerald-600" aria-label="completed" />
                ) : (
                  <Circle className="size-4 text-[var(--muted)]" aria-label="not completed" />
                )}
                {t(`setup.${id}.title`)}
              </li>
            ))}
          </ul>
        </div>
      )}

      {!done && (
        <Button type="button" onClick={onFinish}>
          {t("setup.finish.finishButton")}
        </Button>
      )}

      {done && (
        <div className="space-y-2">
          <Notice tone="ok">{t("setup.finish.finished")}</Notice>
          <p className="text-sm text-[var(--muted)]">{t("setup.finish.revisit")}</p>
        </div>
      )}
    </div>
  );
}
