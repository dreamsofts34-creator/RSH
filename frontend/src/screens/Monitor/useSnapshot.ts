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
 * - `seqGaps` counts dropped snapshots from the raw 30 Hz stream
 *   (`seq` jumps), not from the throttled display copy — throttling
 *   skips snapshots on purpose and must never look like packet loss.
 */
export interface MonitorSnapshot {
  snapshotRef: RefObject<EngineSnapshot | null>;
  snapshot: EngineSnapshot | null;
  seqGaps: number;
}

const DISPLAY_MS = 100;

export function useMonitorSnapshot(): MonitorSnapshot {
  const snapshotRef = useRef<EngineSnapshot | null>(null);
  const [snapshot, setSnapshot] = useState<EngineSnapshot | null>(null);
  const [seqGaps, setSeqGaps] = useState(0);
  const prevSeqRef = useRef<number | null>(null);
  const gapsRef = useRef(0);
  const lastDisplayRef = useRef(0);

  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    subscribeEngineSnapshot((snap) => {
      if (cancelled) return;
      snapshotRef.current = snap;
      // Sequence-gap accounting on the raw stream.
      const prev = prevSeqRef.current;
      if (prev !== null && snap.seq > prev + 1) {
        gapsRef.current += snap.seq - prev - 1;
      }
      // A lower seq means the board restarted its counter; reset, no gap.
      if (prev === null || snap.seq >= prev) {
        prevSeqRef.current = snap.seq;
      } else {
        prevSeqRef.current = snap.seq;
      }
      const now = Date.now();
      if (now - lastDisplayRef.current >= DISPLAY_MS) {
        lastDisplayRef.current = now;
        setSnapshot(snap);
        setSeqGaps(gapsRef.current);
      }
    })
      .then((fn) => {
        unlisten = fn;
      })
      .catch(() => {
        // Event not available (outside Tauri / backend not emitting yet).
      });
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);

  return { snapshotRef, snapshot, seqGaps };
}
