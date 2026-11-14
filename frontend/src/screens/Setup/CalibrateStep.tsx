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
import { Notice, NotImplementedNotice, StepIntro, describeError } from "./components";

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

export function CalibrateStep() {
  const { t } = useTranslation();
  const [loaded, setLoaded] = useState<Calibration | null>(null);
  const [form, setForm] = useState<CalibrationForm>(emptyForm);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<{ notImplemented: boolean; message: string } | null>(
    null,
  );
  const [saved, setSaved] = useState(false);

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

  function handleSave() {
    const steeringCenterMv = Number(form.steeringCenterMv);
    const pedalMinMv: Record<string, number> = {};
    const pedalMaxMv: Record<string, number> = {};
    for (const pedal of PEDALS) {
      pedalMinMv[pedal] = Number(form.pedalMinMv[pedal]);
      pedalMaxMv[pedal] = Number(form.pedalMaxMv[pedal]);
    }
    const allNumbers = [steeringCenterMv, ...Object.values(pedalMinMv), ...Object.values(pedalMaxMv)];
    if (allNumbers.some((value) => !Number.isFinite(value))) {
      setError({ notImplemented: false, message: t("setup.calibrate.invalidNumber") });
      return;
    }
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
    setForm(loaded === null ? emptyForm() : toForm(loaded));
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
          value={form.steeringCenterMv}
          onChange={(event) =>
            setForm((current) => ({ ...current, steeringCenterMv: event.target.value }))
          }
        />
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
                value={form.pedalMinMv[pedal]}
                onChange={(event) =>
                  setForm((current) => ({
                    ...current,
                    pedalMinMv: { ...current.pedalMinMv, [pedal]: event.target.value },
                  }))
                }
              />
            </div>
            <div className="space-y-2">
              <Label htmlFor={`setup-${pedal}-max`}>{t("setup.calibrate.max")}</Label>
              <Input
                id={`setup-${pedal}-max`}
                type="number"
                inputMode="numeric"
                value={form.pedalMaxMv[pedal]}
                onChange={(event) =>
                  setForm((current) => ({
                    ...current,
                    pedalMaxMv: { ...current.pedalMaxMv, [pedal]: event.target.value },
                  }))
                }
              />
            </div>
          </div>
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
      {error?.notImplemented ? (
        <NotImplementedNotice />
      ) : error ? (
        <Notice tone="error">{error.message}</Notice>
      ) : null}
    </div>
  );
}
