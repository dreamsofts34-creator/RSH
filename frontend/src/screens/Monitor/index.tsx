import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Activity } from "lucide-react";
import {
  subscribeButtonPress,
  subscribeConfigRollback,
  subscribeGameState,
  subscribePairingWindow,
  type ConnectionState,
} from "@/lib/tauri";
import { EncoderPulseTimers } from "@/lib/encoderPulse";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { LiveCharts } from "./components/LiveCharts";
import { PacketStats } from "./components/PacketStats";
import { ButtonGrid } from "./components/ButtonGrid";
import { EventLog, type LogEntry } from "./components/EventLog";
import { useMonitorSnapshot } from "./useSnapshot";

export const path = "monitor";
export const label = "Monitor";
export const icon = Activity;
export const component = MonitorScreen;

const LOG_CAP = 100;

/**
 * Monitor (UI plan P8): live uPlot charts, packet stats, button state
 * grid, encoder section, and the event log.
 *
 * Honest gaps (no fake data):
 * - Encoders: the section renders `snapshot.encoders` /
 *   `snapshot.encoderButtons` (M12) — before the first snapshot it
 *   says so instead of showing invented counters.
 * - Buttons: only buttons the `rsh://button-press` event has reported
 *   appear in the grid; nothing is pre-filled. Encoder buttons 65–72
 *   are pulses and auto-clear ~200 ms after the press (C5).
 * - The event log records only events the backend actually emitted
 *   (connection changes, game state, config rollback, pairing window).
 */
export function MonitorScreen() {
  const { t } = useTranslation();
  const { snapshotRef, snapshot } = useMonitorSnapshot();
  const [buttons, setButtons] = useState<Record<number, boolean>>({});
  const [log, setLog] = useState<LogEntry[]>([]);
  const logIdRef = useRef(0);
  const prevConnectionRef = useRef<ConnectionState | null>(null);
  const prevPairingSecondsRef = useRef<number>(0);
  const pulseTimersRef = useRef<EncoderPulseTimers | null>(null);
  if (pulseTimersRef.current === null) {
    pulseTimersRef.current = new EncoderPulseTimers();
  }

  const appendLog = useCallback((text: string) => {
    const id = ++logIdRef.current;
    const time = new Date().toLocaleTimeString();
    setLog((prev) => [{ id, time, text }, ...prev].slice(0, LOG_CAP));
  }, []);

  // Button-press events -> live grid state (M11 done-pattern: a
  // subscription that resolves after unmount is stopped at once).
  useEffect(() => {
    let done = false;
    let unlisten: (() => void) | undefined;
    const pulses = pulseTimersRef.current;
    void subscribeButtonPress(({ button, pressed }) => {
      if (done) return;
      setButtons((prev) => ({ ...prev, [button]: pressed }));
      // Encoder buttons are pulses: no release event ever arrives,
      // so auto-clear ~200 ms after the press (C5).
      if (pressed) {
        pulses?.arm(button, () => {
          if (!done) {
            setButtons((prev) => ({ ...prev, [button]: false }));
          }
        });
      }
    })
      .then((stop) => {
        if (done) stop();
        else unlisten = stop;
      })
      .catch(() => {
        // Event not available yet; the grid shows its empty state.
      });
    return () => {
      done = true;
      pulses?.disarmAll();
      unlisten?.();
    };
  }, []);

  // Game state + config rollback + pairing window -> event log.
  useEffect(() => {
    let done = false;
    const unlistens: (() => void)[] = [];
    const collect = (promise: Promise<() => void>) => {
      void promise
        .then((fn) => {
          if (done) fn();
          else unlistens.push(fn);
        })
        .catch(() => undefined);
    };
    collect(
      subscribeGameState((game) => {
        if (done) return;
        appendLog(
          t(game.running ? "monitor.log.gameStarted" : "monitor.log.gameStopped", {
            name: game.name,
          }),
        );
      }),
    );
    collect(
      subscribeConfigRollback(({ reason }) => {
        if (done) return;
        appendLog(t("monitor.log.rollback", { reason }));
      }),
    );
    collect(
      subscribePairingWindow((status) => {
        if (done) return;
        const prev = prevPairingSecondsRef.current;
        prevPairingSecondsRef.current = status.secondsLeft;
        if (prev === 0 && status.secondsLeft > 0) {
          appendLog(t("monitor.log.pairingOpened"));
        } else if (prev > 0 && status.secondsLeft === 0) {
          appendLog(t("monitor.log.pairingClosed"));
        }
      }),
    );
    return () => {
      done = true;
      for (const fn of unlistens) fn();
    };
  }, [appendLog, t]);

  // Connection changes (from the throttled snapshot) -> event log.
  useEffect(() => {
    if (!snapshot) return;
    const prev = prevConnectionRef.current;
    prevConnectionRef.current = snapshot.connection;
    if (prev !== null && prev !== snapshot.connection) {
      appendLog(
        t("monitor.log.connectionChanged", { state: snapshot.connection }),
      );
    }
  }, [snapshot, appendLog, t]);

  const encoders = snapshot?.encoders ?? null;
  const encoderButtons = snapshot?.encoderButtons ?? [];

  return (
    <section aria-label={t("monitor.title")} className="space-y-5">
      <header>
        <h1 className="text-[26px] font-bold leading-tight">
          {t("monitor.title")}
        </h1>
        <p className="mt-1 text-sm text-[var(--muted)]">
          {t("monitor.subtitle")}
        </p>
      </header>

      <LiveCharts snapshotRef={snapshotRef} />

      <PacketStats
        stats={snapshot?.stats ?? null}
        seqGap={snapshot ? snapshot.seqGap : null}
        txFail={snapshot ? snapshot.txFail : null}
      />

      <div className="grid gap-5 lg:grid-cols-2">
        <ButtonGrid buttons={buttons} />
        <Card>
          <CardHeader>
            <CardTitle>{t("monitor.encoders.title")}</CardTitle>
          </CardHeader>
          <CardContent>
            {snapshot === null ? (
              <p className="text-sm text-[var(--muted)]">
                {t("monitor.encoders.notAvailable")}
              </p>
            ) : encoders === null || encoders.length === 0 ? (
              <p className="text-sm text-[var(--muted)]">
                {t("monitor.encoders.empty")}
              </p>
            ) : (
              <ul className="space-y-2">
                {encoders.map((encoder) => (
                  <li
                    key={encoder.index}
                    className="flex flex-wrap items-center justify-between gap-2 rounded-[10px] border border-[var(--line)] px-3 py-2 text-sm"
                  >
                    <span className="font-semibold">
                      {t("monitor.encoders.encoder", {
                        index: encoder.index + 1,
                        defaultValue: `Encoder ${encoder.index + 1}`,
                      })}
                    </span>
                    <span className="font-mono text-[13px]">
                      {t("monitor.encoders.count")}: {encoder.count}
                      {" · "}
                      {t("monitor.encoders.delta")}:{" "}
                      {encoder.delta > 0 ? `+${encoder.delta}` : encoder.delta}
                    </span>
                    <span className="flex gap-2">
                      <span
                        className={
                          encoder.cwPressed
                            ? "font-semibold text-[var(--success-text)]"
                            : "text-[var(--muted)]"
                        }
                      >
                        {t("monitor.encoders.cw")}: #{encoder.cwButton}
                      </span>
                      <span
                        className={
                          encoder.ccwPressed
                            ? "font-semibold text-[var(--success-text)]"
                            : "text-[var(--muted)]"
                        }
                      >
                        {t("monitor.encoders.ccw")}: #{encoder.ccwButton}
                      </span>
                    </span>
                  </li>
                ))}
                {encoderButtons.length > 0 && (
                  <li className="text-sm text-[var(--muted)]">
                    {t("monitor.encoders.pressed")}:{" "}
                    {encoderButtons.map((b) => `#${b}`).join(", ")}
                  </li>
                )}
              </ul>
            )}
          </CardContent>
        </Card>
      </div>

      <EventLog entries={log} />
    </section>
  );
}
