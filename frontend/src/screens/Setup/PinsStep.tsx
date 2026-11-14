import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import {
  getPinMap,
  validatePinMap,
  type PinAssignment,
  type PinMapValidation,
} from "@/lib/tauri";
import { Notice, NotImplementedNotice, StepIntro, describeError, type StepProps } from "./components";

/**
 * Step 4 — Pins (UI plan P5.3).
 *
 * Read-only view of the board's GPIO pin map, loaded with `get_pin_map`
 * and checkable with `validate_pin_map`. Editing lives in Settings →
 * GPIO (P9.2); this step never writes pins.
 */

export function PinsStep({ onSuccessChange }: StepProps) {
  const { t } = useTranslation();
  const [pins, setPins] = useState<PinAssignment[] | null>(null);
  const [validation, setValidation] = useState<PinMapValidation | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<{ notImplemented: boolean; message: string } | null>(
    null,
  );

  // H5: the step succeeds once the map loads and validates clean.
  useEffect(() => {
    onSuccessChange?.(pins !== null && validation?.valid === true);
  }, [pins, validation, onSuccessChange]);

  useEffect(() => {
    let cancelled = false;
    setBusy(true);
    getPinMap()
      .then((next) => {
        if (!cancelled) setPins(next);
      })
      .catch((caught: unknown) => {
        if (!cancelled) setError(describeError(caught));
      })
      .finally(() => {
        if (!cancelled) setBusy(false);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  function handleValidate() {
    if (pins === null) return;
    setBusy(true);
    setError(null);
    validatePinMap({ pins })
      .then(setValidation)
      .catch((caught: unknown) => setError(describeError(caught)))
      .finally(() => setBusy(false));
  }

  return (
    <div className="space-y-4">
      <StepIntro
        title={t("setup.pins.title")}
        description={t("setup.pins.description")}
      />

      {busy && pins === null && !error && (
        <Notice tone="info">{t("common.loading")}</Notice>
      )}

      {pins !== null && pins.length === 0 && (
        <Notice tone="info">{t("setup.pins.empty")}</Notice>
      )}

      {pins !== null && pins.length > 0 && (
        <table className="w-full text-sm text-[var(--ink)]">
          <thead>
            <tr className="border-b border-[var(--line)] text-left text-[var(--muted)]">
              <th className="py-2 pr-2 font-semibold">{t("setup.pins.function")}</th>
              <th className="py-2 font-semibold">{t("setup.pins.pin")}</th>
            </tr>
          </thead>
          <tbody>
            {pins.map((pin) => (
              <tr key={pin.function} className="border-b border-[var(--line)]">
                <td className="py-2 pr-2">{pin.function}</td>
                <td className="py-2">
                  {pin.pins && pin.pins.length > 0
                    ? pin.pins.map((gpio) => `GPIO ${gpio}`).join(", ")
                    : `GPIO ${pin.pin}`}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}

      {pins !== null && (
        <Button type="button" variant="outline" onClick={handleValidate} disabled={busy}>
          {t("setup.pins.validate")}
        </Button>
      )}

      {validation !== null && (
        <div className="space-y-2">
          {validation.valid ? (
            <Notice tone="ok">{t("setup.pins.valid")}</Notice>
          ) : (
            <div className="space-y-2">
              <p className="text-sm font-semibold text-[var(--ink)]">
                {t("setup.pins.errors")}
              </p>
              <ul className="list-disc space-y-1 pl-5 text-sm text-[var(--ink)]">
                {validation.errors.map((message) => (
                  <li key={message}>{message}</li>
                ))}
              </ul>
            </div>
          )}
          {validation.warnings.length > 0 && (
            <div className="space-y-2">
              <p className="text-sm font-semibold text-[var(--ink)]">
                {t("setup.pins.warnings")}
              </p>
              <ul className="list-disc space-y-1 pl-5 text-sm text-[var(--ink)]">
                {validation.warnings.map((message) => (
                  <li key={message}>{message}</li>
                ))}
              </ul>
            </div>
          )}
        </div>
      )}

      {error?.notImplemented ? (
        <NotImplementedNotice />
      ) : error ? (
        <Notice tone="error">{error.message}</Notice>
      ) : null}
    </div>
  );
}
