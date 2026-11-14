import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { AlertTriangle, CircleAlert, CircleCheck } from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Badge } from "@/components/ui/badge";
import { Label } from "@/components/ui/label";
import {
  confirmConfig,
  getPinMap,
  rollbackConfig,
  setPinMap,
  TauriCommandError,
  validatePinMap,
  type PinAssignment,
  type PinMapValidation,
} from "@/lib/tauri";
import { MODULE_IDS, moduleLabelKey, st, type ModuleId } from "./settings";

/**
 * GPIO pin map editor (UI plan P9.2).
 *
 * The Rust PIN-RULES validator (`rsh_protocol::validate_blob`, reached
 * through the `validate_pin_map` wrapper) is the single authority: this
 * file contains NO pin rules of its own — no ADC lists, no strapping
 * lists, no duplicate checks. A draft is validated by the backend
 * (debounced 400 ms, R-P4) and Apply is enabled only when the validator
 * reports zero errors. A bad map is never sent to the board.
 *
 * `get_pin_map` / `set_pin_map` are implemented (H5-control, one
 * fresh session each), so the editor loads the board's live map on
 * open and applies the full draft to the board. When the board cannot
 * be reached the load fails honestly instead of silently showing an
 * empty draft; the `notImplemented` fallbacks below only trigger
 * against an older backend that predates those commands. The editor
 * is never bricked by a failed load, and a failed Apply never wipes
 * the draft.
 *
 * Apply flow: `set_pin_map` -> confirm bar (30 s) -> `confirm_config`
 * or `rollback_config`. If the 30 s window expires the backend restores
 * the previous map and the editor reloads it (when the board map can
 * be read; otherwise the local draft is kept).
 */

type LoadStatus = "loading" | "ready" | "error";

interface DraftPin {
  fn: ModuleId;
  pin: number | null;
}

const GPIO_PINS: readonly number[] = Array.from({ length: 40 }, (_, i) => i);
const VALIDATION_DEBOUNCE_MS = 400;
const CONFIRM_WINDOW_SECONDS = 30;

function draftFromAssignments(list: PinAssignment[]): DraftPin[] {
  const byFn = new Map<string, number>(
    list.map((a) => [a.function, a.pin] as [string, number]),
  );
  return MODULE_IDS.map((fn) => ({ fn, pin: byFn.get(fn) ?? null }));
}

function assignedPins(draft: DraftPin[]): PinAssignment[] {
  return draft
    .filter((d): d is { fn: ModuleId; pin: number } => d.pin !== null)
    .map((d) => ({ function: d.fn, pin: d.pin }));
}

/**
 * Attach a validator message to the rows it is about: a message names a
 * function (id or label) or a GPIO number. Anything that matches no row
 * is still shown in the summary list below — nothing is swallowed.
 */
function rowMessages(
  messages: string[],
  fn: ModuleId,
  label: string,
  pin: number | null,
): string[] {
  return messages.filter((message) => {
    const lower = message.toLowerCase();
    if (lower.includes(fn) || lower.includes(label.toLowerCase())) return true;
    if (pin !== null) {
      return (
        lower.includes(`gpio ${pin}`) ||
        lower.includes(`pin ${pin}`) ||
        lower.includes(`gpio${pin}`)
      );
    }
    return false;
  });
}

export function PinMapEditor() {
  const { t } = useTranslation();
  const [loadStatus, setLoadStatus] = useState<LoadStatus>("loading");
  const [draft, setDraft] = useState<DraftPin[]>(() =>
    MODULE_IDS.map((fn) => ({ fn, pin: null })),
  );
  const [validation, setValidation] = useState<PinMapValidation | null>(null);
  const [validating, setValidating] = useState(false);
  const [validationUnavailable, setValidationUnavailable] = useState(false);
  /** True when the board's map could not be read (older backend
   *  without `get_pin_map`, or no board): the editor works from an
   *  empty draft instead of showing a dead screen. */
  const [boardMapUnavailable, setBoardMapUnavailable] = useState(false);
  /** True when applying to the board failed as not implemented
   *  (older backend without `set_pin_map`). */
  const [applyUnavailable, setApplyUnavailable] = useState(false);
  const [applying, setApplying] = useState(false);
  const [confirmVisible, setConfirmVisible] = useState(false);
  const [secondsLeft, setSecondsLeft] = useState(CONFIRM_WINDOW_SECONDS);
  const [status, setStatus] = useState<string | null>(null);
  const seqRef = useRef(0);

  const loadPinMap = useCallback(async () => {
    setLoadStatus("loading");
    try {
      const pins = await getPinMap();
      setDraft(draftFromAssignments(pins));
      setBoardMapUnavailable(false);
      setLoadStatus("ready");
    } catch (error) {
      if (error instanceof TauriCommandError && error.notImplemented) {
        // Older backend without `get_pin_map` — start from an empty
        // draft. Validation (`validate_pin_map`) works, so the editor
        // stays usable; only the board push is missing.
        setDraft(MODULE_IDS.map((fn) => ({ fn, pin: null })));
        setBoardMapUnavailable(true);
        setLoadStatus("ready");
      } else {
        setLoadStatus("error");
      }
    }
  }, []);

  useEffect(() => {
    void loadPinMap();
  }, [loadPinMap]);

  // Debounced validation of the current draft (R-P4: no IPC per keystroke).
  useEffect(() => {
    if (loadStatus !== "ready" || validationUnavailable) return;
    setValidating(true);
    const seq = ++seqRef.current;
    const timer = window.setTimeout(() => {
      void validatePinMap({ pins: assignedPins(draft) })
        .then((result) => {
          if (seqRef.current !== seq) return;
          setValidation(result);
          setValidating(false);
        })
        .catch((error: unknown) => {
          if (seqRef.current !== seq) return;
          setValidating(false);
          if (
            error instanceof TauriCommandError &&
            error.notImplemented
          ) {
            setValidationUnavailable(true);
          } else {
            setStatus(
              st(
                t,
                "settings.pins.validationFailed",
                "Could not validate the pin map.",
              ),
            );
          }
        });
    }, VALIDATION_DEBOUNCE_MS);
    return () => window.clearTimeout(timer);
  }, [draft, loadStatus, validationUnavailable, t]);

  // 30 s confirm countdown; expiry means the backend rolled back.
  useEffect(() => {
    if (!confirmVisible) return;
    if (secondsLeft <= 0) {
      setConfirmVisible(false);
      setStatus(
        st(
          t,
          "settings.pins.confirmExpired",
          "Confirmation window expired — the previous pin map was restored.",
        ),
      );
      // Only reload when the board map can be read; otherwise the
      // reload would wipe the local draft back to empty.
      if (!boardMapUnavailable) void loadPinMap();
      return;
    }
    const id = window.setInterval(() => {
      setSecondsLeft((s) => Math.max(0, s - 1));
    }, 1000);
    return () => window.clearInterval(id);
  }, [confirmVisible, secondsLeft, boardMapUnavailable, loadPinMap, t]);

  const applyEnabled =
    loadStatus === "ready" &&
    !validationUnavailable &&
    !applyUnavailable &&
    !validating &&
    !applying &&
    validation !== null &&
    validation.valid &&
    validation.errors.length === 0;

  async function handleApply() {
    if (!applyEnabled) return;
    setApplying(true);
    setStatus(null);
    try {
      const result = await setPinMap({ pins: assignedPins(draft) });
      setValidation(result);
      if (!result.valid || result.errors.length > 0) {
        setStatus(
          st(t, "settings.pins.applyFailed", "Could not apply the pin map."),
        );
        return;
      }
      setConfirmVisible(true);
      setSecondsLeft(CONFIRM_WINDOW_SECONDS);
      setStatus(
        st(
          t,
          "settings.pins.applied",
          "Pin map applied. Confirm within 30 seconds or the previous map is restored.",
        ),
      );
    } catch (error) {
      if (error instanceof TauriCommandError && error.notImplemented) {
        setApplyUnavailable(true);
      } else {
        setStatus(
          st(t, "settings.pins.applyFailed", "Could not apply the pin map."),
        );
      }
    } finally {
      setApplying(false);
    }
  }

  async function handleConfirm() {
    try {
      await confirmConfig();
      setConfirmVisible(false);
      setStatus(
        st(t, "settings.pins.confirmed", "Pin map confirmed."),
      );
    } catch {
      setStatus(
        st(
          t,
          "settings.pins.confirmFailed",
          "Could not confirm the pin map.",
        ),
      );
    }
  }

  async function handleRollback() {
    try {
      await rollbackConfig();
      setConfirmVisible(false);
      setStatus(
        st(t, "settings.pins.rolledBack", "Pin map rolled back."),
      );
      // Only reload when the board map can be read; otherwise the
      // reload would wipe the local draft back to empty.
      if (!boardMapUnavailable) await loadPinMap();
    } catch {
      setStatus(
        st(
          t,
          "settings.pins.rollbackFailed",
          "Could not roll back the pin map.",
        ),
      );
    }
  }

  const errors = validation?.errors ?? [];
  const warnings = validation?.warnings ?? [];

  return (
    <Card>
      <CardHeader>
        <CardTitle>
          {st(t, "settings.pins.title", "GPIO pin map")}
        </CardTitle>
        <CardDescription>
          {st(
            t,
            "settings.pins.description",
            "Assign a GPIO pin to each function. The map is checked by the board validator before it is applied.",
          )}
        </CardDescription>
      </CardHeader>
      <CardContent className="space-y-4">
        {loadStatus === "loading" ? (
          <p className="text-sm text-[var(--muted)]">
            {st(t, "settings.loading", "Loading settings…")}
          </p>
        ) : loadStatus === "error" ? (
          <p role="alert" className="text-sm text-[var(--error-text)]">
            {st(t, "settings.pins.loadFailed", "Could not load the pin map.")}
          </p>
        ) : (
          <>
            {boardMapUnavailable ? (
              <p
                role="status"
                className="rounded-[10px] border border-[var(--line)] bg-[var(--banner)] p-3 text-sm"
              >
                {st(
                  t,
                  "settings.pins.boardMapUnavailable",
                  "Could not read the board's pin map — starting from an empty map. Validation works; applying to the board is not available yet.",
                )}
              </p>
            ) : null}
            {applyUnavailable ? (
              <p role="alert" className="text-sm text-[var(--error-text)]">
                {st(
                  t,
                  "settings.pins.applyUnavailable",
                  "Applying the pin map to the board is not available yet.",
                )}
              </p>
            ) : null}
            <ul className="space-y-3">
              {draft.map((row) => {
                const label = st(
                  t,
                  moduleLabelKey(row.fn),
                  row.fn,
                );
                const rowErrors = rowMessages(errors, row.fn, label, row.pin);
                const rowWarnings = rowMessages(
                  warnings,
                  row.fn,
                  label,
                  row.pin,
                );
                return (
                  <li
                    key={row.fn}
                    className="space-y-1 rounded-[10px] border border-[var(--line)] p-3"
                  >
                    <div className="flex items-center justify-between gap-3">
                      <Label htmlFor={`pin-${row.fn}`}>{label}</Label>
                      <select
                        id={`pin-${row.fn}`}
                        aria-label={st(
                          t,
                          "settings.pins.selectPin",
                          "GPIO pin for {{module}}",
                          { module: label },
                        )}
                        value={row.pin === null ? "" : String(row.pin)}
                        disabled={applying}
                        onChange={(event) => {
                          const value = event.target.value;
                          const pin =
                            value === "" ? null : Number.parseInt(value, 10);
                          setDraft((prev) =>
                            prev.map((d) =>
                              d.fn === row.fn ? { ...d, pin } : d,
                            ),
                          );
                        }}
                        className="h-10 rounded-[10px] border border-[var(--line)] bg-[var(--card)] px-3 py-2 text-sm text-[var(--ink)] focus-visible:outline-2 focus-visible:outline-dashed focus-visible:outline-[var(--accent)] disabled:cursor-not-allowed disabled:opacity-50"
                      >
                        <option value="">
                          {st(
                            t,
                            "settings.pins.unassigned",
                            "Not assigned",
                          )}
                        </option>
                        {GPIO_PINS.map((pin) => (
                          <option key={pin} value={pin}>
                            GPIO {pin}
                          </option>
                        ))}
                      </select>
                    </div>
                    {rowErrors.map((message) => (
                      <p
                        key={message}
                        className="flex items-start gap-1.5 text-[13px] text-[var(--error-text)]"
                      >
                        <CircleAlert
                          className="mt-0.5 h-4 w-4 shrink-0"
                          aria-hidden="true"
                        />
                        {message}
                      </p>
                    ))}
                    {rowWarnings.map((message) => (
                      <p
                        key={message}
                        className="flex items-start gap-1.5 text-[13px] text-[var(--warning-text)]"
                      >
                        <AlertTriangle
                          className="mt-0.5 h-4 w-4 shrink-0"
                          aria-hidden="true"
                        />
                        {message}
                      </p>
                    ))}
                  </li>
                );
              })}
            </ul>

            {validationUnavailable ? (
              <p role="alert" className="text-sm text-[var(--error-text)]">
                {st(
                  t,
                  "settings.pins.validationFailed",
                  "Could not validate the pin map.",
                )}
              </p>
            ) : validating ? (
              <p className="text-sm text-[var(--muted)]">
                {st(t, "settings.pins.validating", "Validating…")}
              </p>
            ) : validation !== null && validation.valid ? (
              <p className="flex items-center gap-1.5 text-sm text-[var(--success-text)]">
                <CircleCheck className="h-4 w-4" aria-hidden="true" />
                {st(t, "settings.pins.valid", "Pin map is valid.")}
              </p>
            ) : null}

            {errors.length > 0 ? (
              <div role="alert" className="space-y-1">
                <p className="text-sm font-semibold text-[var(--error-text)]">
                  {st(
                    t,
                    "settings.pins.errorsTitle",
                    "Validation errors",
                  )}
                </p>
                <ul className="list-disc space-y-1 ps-5 text-[13px] text-[var(--error-text)]">
                  {errors.map((message) => (
                    <li key={message}>{message}</li>
                  ))}
                </ul>
              </div>
            ) : null}

            {warnings.length > 0 ? (
              <div className="space-y-1">
                <p className="text-sm font-semibold text-[var(--warning-text)]">
                  {st(t, "settings.pins.warningsTitle", "Warnings")}
                </p>
                <ul className="list-disc space-y-1 ps-5 text-[13px] text-[var(--warning-text)]">
                  {warnings.map((message) => (
                    <li key={message}>{message}</li>
                  ))}
                </ul>
              </div>
            ) : null}

            {confirmVisible ? (
              <div
                role="alert"
                className="flex flex-wrap items-center gap-2 rounded-[10px] border border-[var(--line)] bg-[var(--banner)] p-3"
              >
                <Badge variant="secondary">
                  {st(
                    t,
                    "settings.pins.secondsLeft",
                    "{{seconds}} seconds left to confirm",
                    { seconds: secondsLeft },
                  )}
                </Badge>
                <div className="flex gap-2">
                  <Button
                    type="button"
                    size="sm"
                    onClick={() => void handleConfirm()}
                  >
                    {st(t, "settings.pins.confirm", "Confirm")}
                  </Button>
                  <Button
                    type="button"
                    size="sm"
                    variant="outline"
                    onClick={() => void handleRollback()}
                  >
                    {st(t, "settings.pins.rollback", "Roll back")}
                  </Button>
                </div>
              </div>
            ) : null}

            <div className="flex gap-2">
              <Button
                type="button"
                disabled={!applyEnabled}
                onClick={() => void handleApply()}
              >
                {st(t, "settings.pins.apply", "Apply pin map")}
              </Button>
              <Button
                type="button"
                variant="outline"
                disabled={applying}
                onClick={() => void loadPinMap()}
              >
                {st(t, "settings.pins.reset", "Reset changes")}
              </Button>
            </div>
          </>
        )}

        {status !== null ? (
          <p aria-live="polite" className="text-sm text-[var(--muted)]">
            {status}
          </p>
        ) : null}
      </CardContent>
    </Card>
  );
}
