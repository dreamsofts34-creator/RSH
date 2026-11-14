import { useEffect, useRef, useState, type RefObject } from "react";
import {
  subscribeEngineSnapshot,
  type EngineSnapshot,
} from "@/lib/tauri";

/**
 * Live 30 Hz engine snapshot feed (`rsh://snapshot`), throttled for
 * display (UI plan R-P2 / R-P6).
 *
 * - `snapshotRef` always holds the LATEST snapshot (30 Hz ingest, no
 *   React re-render) — the uPlot charts read from it.
 * - `snapshot` is the display copy, refreshed at most every 100 ms
 *   (10 Hz), so text readouts re-render at most 10×/s (R-P7).
 * - Packet loss is NOT counted here (C3): the backend reports the
 *   board's own `seqGap`/`txFail` counters in every snapshot — a
 *   `seq` jump counted locally would conflate throttling skips and
 *   restarts with real loss. `PacketStats` renders `snapshot.seqGap`
 *   / `snapshot.txFail` directly.
 */

export interface MonitorSnapshot {
  snapshotRef: RefObject<EngineSnapshot | null>;
  snapshot: EngineSnapshot | null;
}

const DISPLAY_MS = 100;

export function useMonitorSnapshot(): MonitorSnapshot {
  const snapshotRef = useRef<EngineSnapshot | null>(null);
  const [snapshot, setSnapshot] = useState<EngineSnapshot | null>(null);
  const lastDisplayRef = useRef(0);

  useEffect(() => {
    let done = false;
    let unlisten: (() => void) | undefined;
    void subscribeEngineSnapshot((snap) => {
      if (done) return;
      snapshotRef.current = snap;
      const now = Date.now();
      if (now - lastDisplayRef.current >= DISPLAY_MS) {
        lastDisplayRef.current = now;
        setSnapshot(snap);
      }
    })
      .then((stop) => {
        if (done) stop();
        else unlisten = stop;
      })
      .catch(() => {
        // Event not available (outside Tauri / backend not emitting yet).
      });
    return () => {
      done = true;
      unlisten?.();
    };
  }, []);

  return { snapshotRef, snapshot };
}
