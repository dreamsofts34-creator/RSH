import { useEffect, useState } from "react";
import {
  subscribeEngineSnapshot,
  type EngineSnapshot,
} from "@/lib/tauri";

/**
 * Live 30 Hz engine snapshot feed (`rsh://snapshot`).
 *
 * The snapshot is kept in one stable state slot; the Dashboard subtree is
 * the only consumer, so a 30 Hz re-render stays inside this screen
 * (UI plan R-P2 / R-P7). When the backend does not emit yet the snapshot
 * stays `null` — callers render "waiting for live data", never fake data.
 */
export function useEngineSnapshot(): EngineSnapshot | null {
  const [snapshot, setSnapshot] = useState<EngineSnapshot | null>(null);

  useEffect(() => {
    let done = false;
    let unlisten: (() => void) | undefined;
    void subscribeEngineSnapshot((snap) => {
      if (!done) setSnapshot(snap);
    })
      .then((stop) => {
        if (done) stop();
        else unlisten = stop;
      })
      .catch(() => {
        // Event not available (outside Tauri / backend not emitting yet).
        // Snapshot stays null; the screen shows its honest empty state.
      });
    return () => {
      done = true;
      unlisten?.();
    };
  }, []);

  return snapshot;
}
