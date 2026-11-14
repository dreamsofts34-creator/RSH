import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  getCalibration,
  saveCalibration,
  type Calibration,
} from "@/lib/tauri";
import { Notice, NotImplementedNotice, StepIntro, describeError, type StepProps } from "./components";

/**
 * Step 5 — Calibrate (UI plan P5.3).
 *
 * Steering centre + pedal min/max, loaded with `get_calibration` and
 * written back with an explicit `save_calibration` (no debounce — the
 * plan requires an explicit Save). Cancel restores the loaded values.
 * Every value shown comes from the backend; before it loads, the fields
 * are empty rather than filled with invented numbers.
 */

const PEDALS = ["accel", "brake", "clutch"] as const;

type PedalKey = (typeof PEDALS)[number];

interface CalibrationForm {
  steeringCenterMv: string;
  pedalMinMv: Record<PedalKey, string>;
  pedalMaxMv: Record<PedalKey, string>;
}

export interface CalibrationValidation {
  /** Field-level errors, keyed by field id (`steering`, `accel.min`, …). */
  errors: Record<string, string>;
  /**
   * Pedals with BOTH fields empty: not errors — the pedal is missing
   * and is omitted from the save instead of being saved as 0/0 (H10).
   */
  omittedPedals: PedalKey[];
}

export const CALIBRATION_MIN_MV = 0;
export const CALIBRATION_MAX_MV = 3300;

/**
 * Validate a calibration form (H10, pure — unit-tested):
 * - an empty field is invalid (an empty string used to parse as 0);
 * - every value must be a number in 0..3300 mV;
 * - each pedal's min must be below its max;
 * - a pedal with both fields empty is omitted, never saved as 0/0.
 * Error values are message keys into `setup.calibrate.*` (`empty`,
 * `range`, `minMax`); the component translates them.
 */
export function validateCalibrationForm(
  form: CalibrationForm,
): CalibrationValidation {
  const errors: Record<string, string> = {};
  const omittedPedals: PedalKey[] = [];
  const read = (raw: string): number | null => {
    const trimmed = raw.trim();
    if (trimmed === "") return null;
    const value = Number(trimmed);
    return Number.isFinite(value) ? value : null;
  };
  const steering = read(form.steeringCenterMv);
  if (steering === null) {
    errors["steering"] = "empty";
  } else if (steering < CALIBRATION_MIN_MV || steering > CALIBRATION_MAX_MV) {
    errors["steering"] = "range";
  }
  for (const pedal of PEDALS) {
    const minRaw = form.pedalMinMv[pedal].trim();
    const maxRaw = form.pedalMaxMv[pedal].trim();
    if (minRaw === "" && maxRaw === "") {
      omittedPedals.push(pedal);
      continue;
    }
    const min = read(form.pedalMinMv[pedal]);
    const max = read(form.pedalMaxMv[pedal]);
    if (min === null) errors[`${pedal}.min`] = "empty";
    else if (min < CALIBRATION_MIN_MV || min > CALIBRATION_MAX_MV) {
      errors[`${pedal}.min`] = "range";
    }
    if (max === null) errors[`${pedal}.max`] = "empty";
    else if (max < CALIBRATION_MIN_MV || max > CALIBRATION_MAX_MV) {
      errors[`${pedal}.max`] = "range";
    }
    if (min !== null && max !== null && min >= max) {
      errors[pedal] = "minMax";
    }
  }
  return { errors, omittedPedals };
}

function emptyForm(): CalibrationForm {
  return {
    steeringCenterMv: "",
    pedalMinMv: { accel: "", brake: "", clutch: "" },
    pedalMaxMv: { accel: "", brake: "", clutch: "" },
  };
}

function toForm(calibration: Calibration): CalibrationForm {
  return {
    steeringCenterMv: String(calibration.steeringCenterMv),
    pedalMinMv: {
      accel: String(calibration.pedalMinMv.accel ?? ""),
      brake: String(calibration.pedalMinMv.brake ?? ""),
      clutch: String(calibration.pedalMinMv.clutch ?? ""),
    },
    pedalMaxMv: {
      accel: String(calibration.pedalMaxMv.accel ?? ""),
      brake: String(calibration.pedalMaxMv.brake ?? ""),
      clutch: String(calibration.pedalMaxMv.clutch ?? ""),
    },
  };
}

export function CalibrateStep({ onSuccessChange }: StepProps) {
  const { t } = useTranslation();
  const [loaded, setLoaded] = useState<Calibration | null>(null);
  const [form, setForm] = useState<CalibrationForm>(emptyForm);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<{ notImplemented: boolean; message: string } | null>(
    null,
  );
  const [saved, setSaved] = useState(false);

  // H5: the step succeeds once a calibration has been saved.
  useEffect(() => {
    onSuccessChange?.(saved);
  }, [saved, onSuccessChange]);

  useEffect(() => {
    let cancelled = false;
    getCalibration()
      .then((next) => {
        if (cancelled) return;
        setLoaded(next);
        setForm(toForm(next));
      })
      .catch((caught: unknown) => {
        if (!cancelled) setError(describeError(caught));
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const [formErrors, setFormErrors] = useState<Record<string, string>>({});
  const [omittedNote, setOmittedNote] = useState<string | null>(null);

  function handleSave() {
    const { errors, omittedPedals } = validateCalibrationForm(form);
    setFormErrors(errors);
    if (Object.keys(errors).length > 0) {
      const first = Object.values(errors)[0];
      setError({
        notImplemented: false,
        message:
          first === "range"
            ? t("setup.calibrate.rangeError")
            : first === "minMax"
              ? t("setup.calibrate.minMaxError")
              : t("setup.calibrate.emptyError"),
      });
      return;
    }
    const steeringCenterMv = Number(form.steeringCenterMv);
    const pedalMinMv: Record<string, number> = {};
    const pedalMaxMv: Record<string, number> = {};
    for (const pedal of PEDALS) {
      // H10: a pedal left entirely empty is omitted, never saved as 0/0.
      if (omittedPedals.includes(pedal)) continue;
      pedalMinMv[pedal] = Number(form.pedalMinMv[pedal]);
      pedalMaxMv[pedal] = Number(form.pedalMaxMv[pedal]);
    }
    setOmittedNote(
      omittedPedals.length > 0
        ? t("setup.calibrate.omittedPedals", {
            pedals: omittedPedals.join(", "),
          })
        : null,
    );
    setBusy(true);
    setError(null);
    setSaved(false);
    saveCalibration({ calibration: { steeringCenterMv, pedalMinMv, pedalMaxMv } })
      .then((next) => {
        setLoaded(next);
        setForm(toForm(next));
        setSaved(true);
      })
      .catch((caught: unknown) => setError(describeError(caught)))
      .finally(() => setBusy(false));
  }

  function handleCancel() {
    setSaved(false);
    setError(null);
    setFormErrors({});
    setOmittedNote(null);
    setForm(loaded === null ? emptyForm() : toForm(loaded));
  }

  /** Translate a validation error key for one field. */
  function fieldError(field: string): string | null {
    const key = formErrors[field];
    if (!key) return null;
    if (key === "range") return t("setup.calibrate.rangeError");
    if (key === "minMax") return t("setup.calibrate.minMaxError");
    return t("setup.calibrate.emptyError");
  }

  return (
    <div className="space-y-4">
      <StepIntro
        title={t("setup.calibrate.title")}
        description={t("setup.calibrate.description")}
      />

      <div className="space-y-2">
        <Label htmlFor="setup-steering-center">{t("setup.calibrate.steeringCenter")}</Label>
        <Input
          id="setup-steering-center"
          type="number"
          inputMode="numeric"
          aria-invalid={formErrors["steering"] !== undefined}
          value={form.steeringCenterMv}
          onChange={(event) =>
            setForm((current) => ({ ...current, steeringCenterMv: event.target.value }))
          }
        />
        {fieldError("steering") ? (
          <p role="alert" className="text-[13px] text-[var(--error-text)]">
            {fieldError("steering")}
          </p>
        ) : null}
      </div>

      {PEDALS.map((pedal) => (
        <fieldset key={pedal} className="space-y-2 rounded-[10px] border border-[var(--line)] p-3">
          <legend className="px-1 text-sm font-semibold text-[var(--ink)]">
            {t(`setup.calibrate.${pedal}`)}
          </legend>
          <div className="grid grid-cols-2 gap-3">
            <div className="space-y-2">
              <Label htmlFor={`setup-${pedal}-min`}>{t("setup.calibrate.min")}</Label>
              <Input
                id={`setup-${pedal}-min`}
                type="number"
                inputMode="numeric"
                aria-invalid={formErrors[`${pedal}.min`] !== undefined}
                value={form.pedalMinMv[pedal]}
                onChange={(event) =>
                  setForm((current) => ({
                    ...current,
                    pedalMinMv: { ...current.pedalMinMv, [pedal]: event.target.value },
                  }))
                }
              />
              {fieldError(`${pedal}.min`) ? (
                <p role="alert" className="text-[13px] text-[var(--error-text)]">
                  {fieldError(`${pedal}.min`)}
                </p>
              ) : null}
            </div>
            <div className="space-y-2">
              <Label htmlFor={`setup-${pedal}-max`}>{t("setup.calibrate.max")}</Label>
              <Input
                id={`setup-${pedal}-max`}
                type="number"
                inputMode="numeric"
                aria-invalid={formErrors[`${pedal}.max`] !== undefined}
                value={form.pedalMaxMv[pedal]}
                onChange={(event) =>
                  setForm((current) => ({
                    ...current,
                    pedalMaxMv: { ...current.pedalMaxMv, [pedal]: event.target.value },
                  }))
                }
              />
              {fieldError(`${pedal}.max`) ? (
                <p role="alert" className="text-[13px] text-[var(--error-text)]">
                  {fieldError(`${pedal}.max`)}
                </p>
              ) : null}
            </div>
          </div>
          {fieldError(pedal) ? (
            <p role="alert" className="text-[13px] text-[var(--error-text)]">
              {fieldError(pedal)}
            </p>
          ) : null}
        </fieldset>
      ))}

      <div className="flex flex-wrap gap-2">
        <Button type="button" onClick={handleSave} disabled={busy}>
          {t("setup.calibrate.save")}
        </Button>
        <Button type="button" variant="outline" onClick={handleCancel} disabled={busy}>
          {t("setup.calibrate.cancel")}
        </Button>
      </div>

      {saved && <Notice tone="ok">{t("setup.calibrate.saved")}</Notice>}
      {omittedNote !== null ? (
        <Notice tone="info">{omittedNote}</Notice>
      ) : null}
      {error?.notImplemented ? (
        <NotImplementedNotice />
      ) : error ? (
        <Notice tone="error">{error.message}</Notice>
      ) : null}
    </div>
  );
}
