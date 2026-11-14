import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Plug, AlertTriangle, RefreshCw } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Switch } from "@/components/ui/switch";
import { Label } from "@/components/ui/label";
import { Badge } from "@/components/ui/badge";
import { Progress } from "@/components/ui/progress";
import {
  TauriCommandError,
  getOutputState,
  getSnapshot,
  listOutputBackends,
  setOutputBackend,
  setOutputState,
  subscribeButtonPress,
  subscribeEngineSnapshot,
  type BackendStatus,
  type EngineSnapshot,
  type OutputState,
} from "@/lib/tauri";

export const path = "output";
export const label = "Output";
export const icon = Plug;

type LoadState =
  | { status: "loading" }
  | { status: "ready"; state: OutputState }
  | { status: "unavailable"; message: string }
  | { status: "error"; message: string };

export function component() {
  return <OutputScreen />;
}

export function OutputScreen() {
  const { t } = useTranslation();
  const [load, setLoad] = useState<LoadState>({ status: "loading" });
  const [toggling, setToggling] = useState(false);
  const [toggleError, setToggleError] = useState<string | null>(null);
  const [snapshot, setSnapshot] = useState<EngineSnapshot | null>(null);
  const [pressed, setPressed] = useState<ReadonlySet<number>>(new Set());
  // Backend picker (G4): the list comes from the backend — never
  // hardcoded — and the checked radio is always the *live* backend,
  // so a failed switch can never leave the UI claiming a backend
  // that is not actually installed.
  const [backends, setBackends] = useState<BackendStatus[] | null>(null);
  const [backendsUnavailable, setBackendsUnavailable] = useState(false);
  const [switching, setSwitching] = useState<string | null>(null);
  const [switchError, setSwitchError] = useState<string | null>(null);

  const loadState = useCallback(async () => {
    setLoad({ status: "loading" });
    try {
      const state = await getOutputState();
      setLoad({ status: "ready", state });
    } catch (error) {
      if (error instanceof TauriCommandError && error.notImplemented) {
        setLoad({
          status: "unavailable",
          message: t("output.notAvailable", {
            defaultValue: "Output state is not available yet.",
          }),
        });
      } else {
        setLoad({
          status: "error",
          message:
            error instanceof Error
              ? error.message
              : t("output.error", { defaultValue: "Could not load output state." }),
        });
      }
    }
    try {
      setBackends(await listOutputBackends());
      setBackendsUnavailable(false);
    } catch (error) {
      // The picker is an enhancement over the state card above: when
      // the list command does not exist yet, the page still works and
      // says so instead of showing a fake picker.
      setBackends(null);
      setBackendsUnavailable(
        error instanceof TauriCommandError && error.notImplemented,
      );
    }
  }, [t]);

  useEffect(() => {
    void loadState();
  }, [loadState]);

  // Live monitor: 30 Hz snapshot feed + button-press events. No fake data —
  // before the first snapshot arrives the monitor shows nothing live.
  useEffect(() => {
    let cancelled = false;
    const unlistens: (() => void)[] = [];
    void getSnapshot()
      .then((snap) => {
        if (!cancelled) setSnapshot(snap);
      })
      .catch(() => {
        // Snapshot on demand unavailable; the event feed below may still work.
      });
    void subscribeEngineSnapshot((snap) => {
      if (!cancelled) setSnapshot(snap);
    })
      .then((fn) => unlistens.push(fn))
      .catch(() => {});
    void subscribeButtonPress(({ button, pressed: isPressed }) => {
      if (cancelled) return;
      setPressed((prev) => {
        const next = new Set(prev);
        if (isPressed) next.add(button);
        else next.delete(button);
        return next;
      });
    })
      .then((fn) => unlistens.push(fn))
      .catch(() => {});
    return () => {
      cancelled = true;
      for (const fn of unlistens) fn();
    };
  }, []);

  const outputState = load.status === "ready" ? load.state : null;
  // `enabled` when the backend reports it; otherwise `connected` is the
  // only backend-reported on/off signal available.
  const enabled = outputState
    ? (outputState.enabled ?? outputState.connected)
    : false;
  // Fallback in effect: the user selected one backend, a different one
  // is live (the detail string carries the failure reason verbatim).
  const fallbackBackend =
    backends?.find((backend) => backend.selected && !backend.active) ?? null;

  const onToggle = async (next: boolean) => {
    if (!outputState) return;
    setToggling(true);
    setToggleError(null);
    try {
      const state = await setOutputState({ enabled: next });
      setLoad({ status: "ready", state });
    } catch (error) {
      setToggleError(
        error instanceof Error
          ? error.message
          : t("output.toggleError", {
              defaultValue: "Could not change output state.",
            }),
      );
    } finally {
      setToggling(false);
    }
  };

  const onSelectBackend = async (id: string) => {
    if (switching !== null) return;
    setSwitching(id);
    setSwitchError(null);
    try {
      const state = await setOutputBackend({ backend: id });
      setLoad({ status: "ready", state });
      setBackends(await listOutputBackends());
    } catch (error) {
      setSwitchError(
        error instanceof Error
          ? error.message
          : t("output.backendSwitchError", {
              defaultValue: "Could not switch backend.",
            }),
      );
    } finally {
      setSwitching(null);
    }
  };

  const pedals = snapshot?.pedals;
  const steeringPct = snapshot
    ? Math.min(100, Math.abs(snapshot.steeringDeg) / 450) * 100
    : 0;

  return (
    <section aria-label={t("output.title", { defaultValue: "Output" })} className="space-y-6">
      <header>
        <h1 className="text-[26px] font-bold leading-tight">
          {t("output.title", { defaultValue: "Output" })}
        </h1>
        <p className="mt-1 max-w-2xl text-sm text-[var(--muted)]">
          {t("output.description", {
            defaultValue:
              "Live output state and values sent to the game. Output can only be enabled or disabled here.",
          })}
        </p>
      </header>

      {load.status === "loading" ? (
        <p className="text-sm text-[var(--muted)]">
          {t("output.loading", { defaultValue: "Loading output state…" })}
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
            <Button variant="outline" size="sm" onClick={() => void loadState()}>
              <RefreshCw className="h-4 w-4" aria-hidden="true" />
              {t("output.retry", { defaultValue: "Retry" })}
            </Button>
          </CardContent>
        </Card>
      ) : null}

      {outputState ? (
        <Card>
          <CardHeader>
            <CardTitle>
              {t("output.backend", { defaultValue: "Backend" })}
            </CardTitle>
          </CardHeader>
          <CardContent className="space-y-4">
            <div className="flex flex-wrap items-center gap-3">
              <span className="text-lg font-bold">{outputState.backend}</span>
              <Badge variant={outputState.connected ? "success" : "destructive"}>
                {outputState.connected
                  ? t("output.connected", { defaultValue: "Connected" })
                  : t("output.disconnected", { defaultValue: "Disconnected" })}
              </Badge>
              <Badge variant={enabled ? "success" : "outline"}>
                {enabled
                  ? t("output.enabled", { defaultValue: "Output enabled" })
                  : t("output.disabled", { defaultValue: "Disabled" })}
              </Badge>
            </div>
            {outputState.detail ? (
              <p className="text-sm text-[var(--muted)]">{outputState.detail}</p>
            ) : null}
            {!outputState.connected ? (
              <p className="flex items-start gap-2 text-sm text-[var(--warning-text)]">
                <AlertTriangle className="mt-0.5 h-4 w-4 shrink-0" aria-hidden="true" />
                {t("output.backendMissing", {
                  defaultValue:
                    "No output backend is available. Output stays disabled until a backend is connected.",
                })}
              </p>
            ) : null}

            <div className="flex items-center gap-3 border-t border-[var(--line)] pt-4">
              <Switch
                id="output-enabled"
                checked={enabled}
                disabled={toggling || !outputState.connected}
                onCheckedChange={(next) => void onToggle(next)}
                aria-label={t("output.enableToggle", { defaultValue: "Enable output" })}
              />
              <Label htmlFor="output-enabled">
                {t("output.enableToggle", { defaultValue: "Enable output" })}
              </Label>
            </div>
            {toggleError ? (
              <p role="alert" className="text-sm text-[var(--error-text)]">
                {toggleError}
              </p>
            ) : null}
          </CardContent>
        </Card>
      ) : null}

      {outputState && backends ? (
        <Card>
          <CardHeader>
            <CardTitle>
              {t("output.backendPicker", { defaultValue: "Backend selection" })}
            </CardTitle>
          </CardHeader>
          <CardContent className="space-y-4">
            <p className="text-sm text-[var(--muted)]">
              {t("output.backendPickerDescription", {
                defaultValue:
                  "Choose which virtual joystick the wheel talks to. The switch happens immediately — no restart needed.",
              })}
            </p>
            {fallbackBackend ? (
              <p
                role="alert"
                className="flex items-start gap-2 text-sm text-[var(--warning-text)]"
              >
                <AlertTriangle className="mt-0.5 h-4 w-4 shrink-0" aria-hidden="true" />
                <span>
                  {t("output.backendFallback", {
                    defaultValue:
                      "The selected backend could not connect, so RSH is using the fallback instead:",
                  })}
                  <span className="block text-[var(--muted)]">
                    {outputState.detail}
                  </span>
                </span>
              </p>
            ) : null}
            <div
              role="radiogroup"
              aria-label={t("output.backendPicker", {
                defaultValue: "Backend selection",
              })}
              className="space-y-3"
            >
              {backends.map((backend) => (
                <label
                  key={backend.id}
                  className="flex cursor-pointer items-start gap-3 rounded-lg border border-[var(--line)] p-3"
                >
                  <input
                    type="radio"
                    name="output-backend"
                    value={backend.id}
                    checked={backend.active}
                    disabled={switching !== null || backend.active}
                    onChange={() => void onSelectBackend(backend.id)}
                    className="mt-1"
                  />
                  <span className="space-y-1">
                    <span className="flex flex-wrap items-center gap-2">
                      <span className="font-semibold">{backend.name}</span>
                      {backend.isDefault ? (
                        <Badge variant="outline">
                          {t("output.backendDefault", { defaultValue: "Default" })}
                        </Badge>
                      ) : null}
                      {backend.active ? (
                        <Badge variant="success">
                          {t("output.backendActive", { defaultValue: "Active" })}
                        </Badge>
                      ) : null}
                      {backend.active && backend.connected ? (
                        <Badge variant="success">
                          {t("output.connected", { defaultValue: "Connected" })}
                        </Badge>
                      ) : null}
                      {backend.installed === false ? (
                        <Badge variant="destructive">
                          {t("output.backendNotInstalled", {
                            defaultValue: "Not installed",
                          })}
                        </Badge>
                      ) : null}
                    </span>
                    <span className="block text-sm text-[var(--muted)]">
                      {backend.detail}
                    </span>
                    {backend.uacNote ? (
                      <span className="block text-sm text-[var(--warning-text)]">
                        {backend.uacNote}
                      </span>
                    ) : null}
                    {backend.downloadUrl ? (
                      <a
                        className="block text-sm font-semibold underline"
                        href={backend.downloadUrl}
                        target="_blank"
                        rel="noreferrer"
                      >
                        {t("output.backendInstallVJoy", {
                          defaultValue: "Install vJoy",
                        })}
                      </a>
                    ) : null}
                  </span>
                </label>
              ))}
            </div>
            {switchError ? (
              <p role="alert" className="text-sm text-[var(--error-text)]">
                {switchError}
              </p>
            ) : null}
          </CardContent>
        </Card>
      ) : null}

      {outputState && backends === null && backendsUnavailable ? (
        <p className="text-sm text-[var(--muted)]">
          {t("output.backendPickerUnavailable", {
            defaultValue: "Backend list is not available yet.",
          })}
        </p>
      ) : null}

      <Card>
        <CardHeader>
          <CardTitle>
            {t("output.liveValues", { defaultValue: "Live output values" })}
          </CardTitle>
        </CardHeader>
        <CardContent className="space-y-4">
          {snapshot && pedals ? (
            <>
              <div className="space-y-1">
                <div className="flex justify-between text-sm">
                  <span>{t("output.steering", { defaultValue: "Steering" })}</span>
                  <span className="font-semibold">
                    {snapshot.steeringDeg.toFixed(0)}°
                  </span>
                </div>
                <Progress value={steeringPct} aria-label={t("output.steering", { defaultValue: "Steering" })} />
              </div>
              <div className="space-y-1">
                <div className="flex justify-between text-sm">
                  <span>Accel</span>
                  <span className="font-semibold">
                    {Math.round(pedals.accel * 100)}%
                  </span>
                </div>
                <Progress value={pedals.accel * 100} aria-label="Accel" />
              </div>
              <div className="space-y-1">
                <div className="flex justify-between text-sm">
                  <span>Brake</span>
                  <span className="font-semibold">
                    {Math.round(pedals.brake * 100)}%
                  </span>
                </div>
                <Progress value={pedals.brake * 100} aria-label="Brake" />
              </div>
              {pedals.clutch !== null ? (
                <div className="space-y-1">
                  <div className="flex justify-between text-sm">
                    <span>Clutch</span>
                    <span className="font-semibold">
                      {Math.round(pedals.clutch * 100)}%
                    </span>
                  </div>
                  <Progress value={pedals.clutch * 100} aria-label="Clutch" />
                </div>
              ) : (
                <p className="text-sm text-[var(--muted)]">
                  Clutch — {t("output.notPresent", { defaultValue: "Not present" })}
                </p>
              )}
            </>
          ) : (
            <p className="text-sm text-[var(--muted)]">
              {t("output.loading", { defaultValue: "Loading output state…" })}
            </p>
          )}
          <div className="border-t border-[var(--line)] pt-3">
            <p className="text-sm font-semibold">
              {t("output.pressedButtons", { defaultValue: "Buttons pressed" })}
            </p>
            {pressed.size > 0 ? (
              <p className="mt-1 text-sm">
                {[...pressed].sort((a, b) => a - b).map((b) => `#${b}`).join(", ")}
              </p>
            ) : (
              <p className="mt-1 text-sm text-[var(--muted)]">—</p>
            )}
          </div>
        </CardContent>
      </Card>
    </section>
  );
}

export default OutputScreen;
