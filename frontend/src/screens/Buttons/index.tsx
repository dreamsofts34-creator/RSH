import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Gamepad2, AlertTriangle, RefreshCw } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Badge } from "@/components/ui/badge";
import { cn } from "@/lib/utils";
import { EncoderPulseTimers } from "@/lib/encoderPulse";
import {
  TauriCommandError,
  commitConfig,
  confirmConfig,
  getConfig,
  getConfigState,
  rollbackConfig,
  stageConfig,
  subscribeButtonPress,
  type ConfigState,
} from "@/lib/tauri";
import {
  ASSIGNABLE_ACTIONS,
  ENCODER_BUTTONS,
  PHYSICAL_BUTTONS,
  parseBindings,
  withBinding,
  type ButtonBindings,
} from "./bindings";

export const path = "buttons";
export const label = "Buttons";
export const icon = Gamepad2;

type LoadState =
  | { status: "loading" }
  | { status: "ready"; config: Record<string, unknown>; bindings: ButtonBindings }
  | { status: "unavailable"; message: string }
  | { status: "error"; message: string };

export function component() {
  return <ButtonsScreen />;
}

export function ButtonsScreen() {
  const { t } = useTranslation();
  const [load, setLoad] = useState<LoadState>({ status: "loading" });
  const [pressed, setPressed] = useState<ReadonlySet<number>>(new Set());
  const [rebindButton, setRebindButton] = useState<number | null>(null);
  const [chosenAction, setChosenAction] = useState<string>("unbound");
  const [capturing, setCapturing] = useState(false);
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
  /** Seconds left in the backend confirm window after commit (H7). */
  const [confirmSeconds, setConfirmSeconds] = useState<number | null>(null);
  /** True when a step failed after the config changed: offer rollback. */
  const [rollbackOffer, setRollbackOffer] = useState(false);
  const [rollingBack, setRollingBack] = useState(false);
  const capturingRef = useRef(false);
  const rebindRef = useRef<number | null>(null);
  /** Mirror of `load` for the subscription (M11: no re-subscribe on load change). */
  const loadRef = useRef(load);
  loadRef.current = load;
  const pulseTimersRef = useRef<EncoderPulseTimers | null>(null);
  if (pulseTimersRef.current === null) {
    pulseTimersRef.current = new EncoderPulseTimers();
  }

  const actionLabel = (action: string): string =>
    t(`buttons.actions.${action}`, { defaultValue: action });

  const loadBindings = useCallback(async () => {
    setLoad({ status: "loading" });
    try {
      const config = await getConfig();
      setLoad({ status: "ready", config, bindings: parseBindings(config) });
    } catch (error) {
      if (error instanceof TauriCommandError && error.notImplemented) {
        setLoad({ status: "unavailable", message: t("buttons.notAvailable", { defaultValue: "Button bindings are not available yet." }) });
      } else {
        setLoad({
          status: "error",
          message:
            error instanceof Error
              ? error.message
              : t("buttons.error", { defaultValue: "Could not load button bindings." }),
        });
      }
    }
  }, [t]);

  useEffect(() => {
    void loadBindings();
  }, [loadBindings]);

  // Live highlight + press-to-capture from the button-press event feed.
  // M11: subscribed ONCE (no `load.status` dep — the handler reads the
  // load mirror ref instead, so saving a binding never re-subscribes
  // and drops a press mid-capture). C5: encoder buttons 65–72 are
  // pulses and auto-clear ~200 ms after the press.
  useEffect(() => {
    let done = false;
    let unlisten: (() => void) | undefined;
    const pulses = pulseTimersRef.current;
    void subscribeButtonPress(({ button, pressed: isPressed }) => {
      if (done) return;
      setPressed((prev) => {
        const next = new Set(prev);
        if (isPressed) next.add(button);
        else next.delete(button);
        return next;
      });
      if (isPressed) {
        pulses?.arm(button, () => {
          if (done) return;
          setPressed((prev) => {
            const next = new Set(prev);
            next.delete(button);
            return next;
          });
        });
      }
      // Press-to-capture: the next pressed button becomes the rebind target.
      if (isPressed && capturingRef.current) {
        capturingRef.current = false;
        setCapturing(false);
        rebindRef.current = button;
        setRebindButton(button);
        const current = loadRef.current;
        if (current.status === "ready") {
          setChosenAction(current.bindings[button] ?? "unbound");
        }
      }
    })
      .then((stop) => {
        if (done) stop();
        else unlisten = stop;
      })
      .catch(() => {
        // No event feed yet (backend not emitting): the grid still works;
        // live highlight and capture just stay idle. Never fake presses.
      });
    return () => {
      done = true;
      pulses?.disarmAll();
      unlisten?.();
    };
  }, []);

  // Confirm-window countdown (H7): ticks down from the 30 s backend
  // window while a rebind is being confirmed.
  useEffect(() => {
    if (confirmSeconds === null) return undefined;
    if (confirmSeconds <= 0) {
      setConfirmSeconds(null);
      return undefined;
    }
    const timer = window.setTimeout(() => {
      setConfirmSeconds((s) => (s === null ? null : Math.max(0, s - 1)));
    }, 1000);
    return () => window.clearTimeout(timer);
  }, [confirmSeconds]);

  const openRebind = (button: number) => {
    rebindRef.current = button;
    setRebindButton(button);
    setChosenAction(
      load.status === "ready" ? (load.bindings[button] ?? "unbound") : "unbound",
    );
    setCapturing(false);
    capturingRef.current = false;
    setSaveError(null);
  };

  const closeRebind = () => {
    setRebindButton(null);
    setCapturing(false);
    capturingRef.current = false;
    setSaveError(null);
    setRollbackOffer(false);
    setConfirmSeconds(null);
  };

  /** Verify the backend config state after a step (H3/H7): the
   *  step commands return nothing, so the state is the proof. */
  async function expectConfigState(
    step: string,
    expected: ConfigState,
  ): Promise<void> {
    const state = await getConfigState();
    if (state !== expected) {
      throw new Error(
        `${step}: expected config state "${expected}" but the backend reports "${state}".`,
      );
    }
  }

  const saveBinding = async () => {
    if (load.status !== "ready" || rebindButton === null) return;
    setSaving(true);
    setSaveError(null);
    setRollbackOffer(false);
    try {
      const nextConfig = withBinding(
        load.config,
        rebindButton,
        chosenAction === "unbound" ? null : chosenAction,
      );
      await stageConfig({ config: nextConfig });
      await expectConfigState("stage", "staged");
      await commitConfig();
      await expectConfigState("commit", "pending_confirm");
      // The backend confirm window is 30 s: show the countdown while
      // confirming so a slow confirm is visible, not silent.
      setConfirmSeconds(30);
      await confirmConfig();
      await expectConfigState("confirm", "clean");
      setConfirmSeconds(null);
      setLoad({ status: "ready", config: nextConfig, bindings: parseBindings(nextConfig) });
      closeRebind();
    } catch (error) {
      setConfirmSeconds(null);
      // The config may already have changed: offer a rollback instead
      // of leaving a half-applied binding (H7).
      setRollbackOffer(true);
      setSaveError(
        error instanceof Error
          ? error.message
          : t("buttons.saveError", { defaultValue: "Could not save the binding." }),
      );
    } finally {
      setSaving(false);
    }
  };

  const handleRollback = async () => {
    setRollingBack(true);
    setSaveError(null);
    try {
      await rollbackConfig();
      await loadBindings();
      setRollbackOffer(false);
      closeRebind();
    } catch (error) {
      setSaveError(
        error instanceof Error
          ? error.message
          : t("buttons.saveError", { defaultValue: "Could not save the binding." }),
      );
    } finally {
      setRollingBack(false);
    }
  };

  const bindings = load.status === "ready" ? load.bindings : {};
  const rebindBinding =
    rebindButton !== null ? (bindings[rebindButton] ?? null) : null;

  const renderCell = (button: number, sublabel?: string) => {
    const binding = bindings[button];
    const isPressed = pressed.has(button);
    const isRebinding = rebindButton === button;
    return (
      <button
        key={button}
        type="button"
        onClick={() => openRebind(button)}
        aria-label={t("buttons.rebindTitle", { button, defaultValue: `Rebind button ${button}` })}
        aria-pressed={isPressed}
        className={cn(
          "flex min-h-16 flex-col items-start justify-between rounded-[12px] border p-2 text-left transition-colors focus-visible:outline-2 focus-visible:outline-dashed focus-visible:outline-[var(--accent)]",
          isPressed
            ? "border-[var(--accent)] bg-[var(--accent-soft)]"
            : "border-[var(--line)] bg-[var(--card)] hover:border-[var(--accent)]",
          isRebinding && "ring-2 ring-[var(--accent)]",
        )}
      >
        <span className="text-xs font-bold">#{button}</span>
        {sublabel ? (
          <span className="text-[11px] text-[var(--muted)]">{sublabel}</span>
        ) : null}
        <span
          className={cn(
            "text-xs",
            binding ? "font-semibold text-[var(--ink)]" : "text-[var(--muted)]",
          )}
        >
          {binding
            ? actionLabel(binding)
            : t("buttons.unbound", { defaultValue: "Unbound" })}
        </span>
      </button>
    );
  };

  return (
    <section aria-label={t("buttons.title", { defaultValue: "Buttons" })} className="space-y-6">
      <header>
        <h1 className="text-[26px] font-bold leading-tight">
          {t("buttons.title", { defaultValue: "Buttons" })}
        </h1>
        <p className="mt-1 max-w-2xl text-sm text-[var(--muted)]">
          {t("buttons.description", {
            defaultValue:
              "Each button shows the action it triggers. Select a button to rebind it, or press a button on the wheel to find it.",
          })}
        </p>
      </header>

      {load.status === "loading" ? (
        <p className="text-sm text-[var(--muted)]">
          {t("buttons.loading", { defaultValue: "Loading button bindings…" })}
        </p>
      ) : null}

      {load.status === "unavailable" ? (
        <Card>
          <CardContent className="flex items-start gap-3 pt-5">
            <AlertTriangle className="mt-0.5 h-5 w-5 shrink-0 text-[var(--warning-text)]" aria-hidden="true" />
            <p className="text-sm">{load.message}</p>
          </CardContent>
        </Card>
      ) : null}

      {load.status === "error" ? (
        <Card>
          <CardContent className="flex flex-col items-start gap-3 pt-5">
            <p className="text-sm text-[var(--error-text)]">{load.message}</p>
            <Button variant="outline" size="sm" onClick={() => void loadBindings()}>
              <RefreshCw className="h-4 w-4" aria-hidden="true" />
              {t("buttons.retry", { defaultValue: "Retry" })}
            </Button>
          </CardContent>
        </Card>
      ) : null}

      {load.status === "ready" ? (
        <>
          <p className="text-sm text-[var(--muted)]">
            {t("buttons.pressHint", {
              defaultValue: "Press a button on the wheel to highlight it here.",
            })}
          </p>

          <Card>
            <CardHeader>
              <CardTitle>
                {t("buttons.physical", { defaultValue: "Physical buttons" })}
              </CardTitle>
            </CardHeader>
            <CardContent>
              <div className="grid grid-cols-4 gap-2 sm:grid-cols-8">
                {PHYSICAL_BUTTONS.map((button) => renderCell(button))}
              </div>
            </CardContent>
          </Card>

          <Card>
            <CardHeader>
              <CardTitle>
                {t("buttons.encoders", { defaultValue: "Encoder buttons" })}
              </CardTitle>
            </CardHeader>
            <CardContent>
              <div className="grid grid-cols-4 gap-2 sm:grid-cols-8">
                {ENCODER_BUTTONS.map(({ button, encoder, direction }) =>
                  renderCell(
                    button,
                    t(
                      direction === "cw" ? "buttons.encoderCw" : "buttons.encoderCcw",
                      {
                        encoder,
                        defaultValue:
                          direction === "cw"
                            ? `Encoder ${encoder} clockwise`
                            : `Encoder ${encoder} counter-clockwise`,
                      },
                    ),
                  ),
                )}
              </div>
            </CardContent>
          </Card>
        </>
      ) : null}

      <Dialog
        open={rebindButton !== null}
        onOpenChange={(open) => {
          if (!open) closeRebind();
        }}
      >
        <DialogContent aria-label={t("buttons.rebindTitle", { button: rebindButton ?? "", defaultValue: "Rebind button" })}>
          <DialogHeader>
            <DialogTitle>
              {t("buttons.rebindTitle", {
                button: rebindButton ?? "",
                defaultValue: `Rebind button ${rebindButton ?? ""}`,
              })}
            </DialogTitle>
            <DialogDescription>
              {t("buttons.currentBinding", { defaultValue: "Current binding" })}
              {": "}
              {rebindBinding
                ? actionLabel(rebindBinding)
                : t("buttons.unbound", { defaultValue: "Unbound" })}
            </DialogDescription>
          </DialogHeader>

          <div className="space-y-4">
            <fieldset>
              <legend className="mb-2 text-sm font-semibold">
                {t("buttons.chooseAction", { defaultValue: "Choose an action" })}
              </legend>
              <div className="space-y-1" role="radiogroup" aria-label={t("buttons.chooseAction", { defaultValue: "Choose an action" })}>
                <label className="flex cursor-pointer items-center gap-2 rounded-lg border border-[var(--line)] px-3 py-2 text-sm">
                  <input
                    type="radio"
                    name="rebind-action"
                    value="unbound"
                    checked={chosenAction === "unbound"}
                    onChange={() => setChosenAction("unbound")}
                  />
                  {t("buttons.actions.unbound", { defaultValue: "Unbound" })}
                </label>
                {ASSIGNABLE_ACTIONS.map((action) => (
                  <label
                    key={action}
                    className="flex cursor-pointer items-center gap-2 rounded-lg border border-[var(--line)] px-3 py-2 text-sm"
                  >
                    <input
                      type="radio"
                      name="rebind-action"
                      value={action}
                      checked={chosenAction === action}
                      onChange={() => setChosenAction(action)}
                    />
                    {actionLabel(action)}
                  </label>
                ))}
              </div>
            </fieldset>

            <Button
              type="button"
              variant={capturing ? "default" : "outline"}
              onClick={() => {
                capturingRef.current = true;
                setCapturing(true);
              }}
              disabled={capturing}
            >
              {capturing
                ? t("buttons.capturing", { defaultValue: "Press any button on the wheel…" })
                : t("buttons.capture", { defaultValue: "Press a button on the wheel" })}
            </Button>

            {capturing ? (
              <Badge variant="secondary">
                {t("buttons.capturing", { defaultValue: "Press any button on the wheel…" })}
              </Badge>
            ) : null}

            {confirmSeconds !== null ? (
              <p role="status" className="text-sm text-[var(--muted)]">
                {t("buttons.confirmCountdown", {
                  seconds: confirmSeconds,
                  defaultValue: `${confirmSeconds} seconds left to confirm`,
                })}
              </p>
            ) : null}

            {saveError ? (
              <p role="alert" className="text-sm text-[var(--error-text)]">
                {saveError}
              </p>
            ) : null}

            {rollbackOffer ? (
              <div className="space-y-2 rounded-[10px] border border-[var(--line)] bg-[var(--banner)] p-3">
                <p className="text-sm">
                  {t("buttons.rollbackOffer", {
                    defaultValue:
                      "The binding may not have saved. Roll back to the previous bindings?",
                  })}
                </p>
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  disabled={rollingBack}
                  onClick={() => void handleRollback()}
                >
                  {rollingBack
                    ? t("buttons.rollingBack", { defaultValue: "Rolling back…" })
                    : t("buttons.rollback", { defaultValue: "Roll back" })}
                </Button>
              </div>
            ) : null}
          </div>

          <DialogFooter>
            <Button type="button" variant="outline" onClick={closeRebind}>
              {t("buttons.cancel", { defaultValue: "Cancel" })}
            </Button>
            <Button type="button" onClick={() => void saveBinding()} disabled={saving}>
              {saving
                ? t("buttons.saving", { defaultValue: "Saving…" })
                : t("buttons.save", { defaultValue: "Save binding" })}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </section>
  );
}

export default ButtonsScreen;
