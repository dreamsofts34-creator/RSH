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
 * - Encoders: the backend exposes no encoder event or command, so the
 *   section says so instead of showing invented counters.
 * - Buttons: only buttons the `rsh://button-press` event has reported
 *   appear in the grid; nothing is pre-filled.
 * - The event log records only events the backend actually emitted
 *   (connection changes, game state, config rollback, pairing window).
 */
export function MonitorScreen() {
  const { t } = useTranslation();
  const { snapshotRef, snapshot, seqGaps } = useMonitorSnapshot();
  const [buttons, setButtons] = useState<Record<number, boolean>>({});
  const [log, setLog] = useState<LogEntry[]>([]);
  const logIdRef = useRef(0);
  const prevConnectionRef = useRef<ConnectionState | null>(null);
  const prevPairingSecondsRef = useRef<number>(0);

  const appendLog = useCallback((text: string) => {
    const id = ++logIdRef.current;
    const time = new Date().toLocaleTimeString();
    setLog((prev) => [{ id, time, text }, ...prev].slice(0, LOG_CAP));
  }, []);

  // Button-press events -> live grid state.
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    subscribeButtonPress(({ button, pressed }) => {
      setButtons((prev) => ({ ...prev, [button]: pressed }));
    })
      .then((fn) => {
        unlisten = fn;
      })
      .catch(() => {
        // Event not available yet; the grid shows its empty state.
      });
    return () => {
      unlisten?.();
    };
  }, []);

  // Game state + config rollback + pairing window -> event log.
  useEffect(() => {
    const unlistens: (() => void)[] = [];
    subscribeGameState((game) => {
      appendLog(
        t(game.running ? "monitor.log.gameStarted" : "monitor.log.gameStopped", {
          name: game.name,
        }),
      );
    })
      .then((fn) => unlistens.push(fn))
      .catch(() => undefined);
    subscribeConfigRollback(({ reason }) => {
      appendLog(t("monitor.log.rollback", { reason }));
    })
      .then((fn) => unlistens.push(fn))
      .catch(() => undefined);
    subscribePairingWindow((status) => {
      const prev = prevPairingSecondsRef.current;
      prevPairingSecondsRef.current = status.secondsLeft;
      if (prev === 0 && status.secondsLeft > 0) {
        appendLog(t("monitor.log.pairingOpened"));
      } else if (prev > 0 && status.secondsLeft === 0) {
        appendLog(t("monitor.log.pairingClosed"));
      }
    })
      .then((fn) => unlistens.push(fn))
      .catch(() => undefined);
    return () => {
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

      <PacketStats stats={snapshot?.stats ?? null} seqGaps={seqGaps} />

      <div className="grid gap-5 lg:grid-cols-2">
        <ButtonGrid buttons={buttons} />
        <Card>
          <CardHeader>
            <CardTitle>{t("monitor.encoders.title")}</CardTitle>
          </CardHeader>
          <CardContent>
            <p className="text-sm text-[var(--muted)]">
              {t("monitor.encoders.notAvailable")}
            </p>
          </CardContent>
        </Card>
      </div>

      <EventLog entries={log} />
    </section>
  );
}
