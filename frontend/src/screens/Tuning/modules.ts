import type { ComponentType } from "react";
import {
  ArrowUpDown,
  Disc,
  Footprints,
  Gauge,
  Hand,
  LayoutGrid,
  LifeBuoy,
  Lightbulb,
  Megaphone,
  RotateCw,
} from "lucide-react";
import type { EngineSnapshot } from "@/lib/tauri";

/**
 * The 10 tunable modules (UI plan P4.1), in the approved order.
 *
 * Module on/off state: UI plan §0 golden rule 2 + §P9.1 put the master
 * toggles ONLY in Settings ("Tuning cards sirf state dikhate hain, toggle
 * nahi karte"), so this screen NEVER renders a toggle. It reads the same
 * state the Settings toggles write, through the same `get_settings`
 * wrapper — a single source of truth, not a second one. The state lives
 * in the settings record under the `modules` key:
 * `{ "modules": { "steering": true, "brake": false, ... } }`.
 * That key is a PROPOSED contract for the Settings worker to confirm;
 * if it is absent (e.g. the backend is not implemented yet) every module
 * reports "Unknown" instead of a faked on/off state.
 *
 * Live input: only steering + the three pedals have a snapshot channel
 * (EngineSnapshot). The other modules report no live value — never a
 * faked one.
 */

export type ModuleId =
  | "steering"
  | "accelerator"
  | "brake"
  | "clutch"
  | "handbrake"
  | "shifter"
  | "horn"
  | "buttons"
  | "encoders"
  | "led";

export interface ModuleDef {
  id: ModuleId;
  /** i18n key for the card/panel title (`tuning.modules.<id>.label`). */
  labelKey: string;
  /** i18n key for the one-line card hint (`tuning.modules.<id>.hint`). */
  hintKey: string;
  icon: ComponentType<{ className?: string }>;
  /**
   * Live input 0..1 from a snapshot, or null when this module has no
   * snapshot channel (no live marker / mini value is shown then).
   */
  liveValueOf: ((snapshot: EngineSnapshot) => number | null) | null;
}

function clamp01(value: number): number {
  return Math.min(1, Math.max(0, value));
}

export const MODULES: readonly ModuleDef[] = [
  {
    id: "steering",
    labelKey: "tuning.modules.steering.label",
    hintKey: "tuning.modules.steering.hint",
    icon: LifeBuoy,
    liveValueOf: (snapshot) => clamp01((snapshot.steeringDeg + 450) / 900),
  },
  {
    id: "accelerator",
    labelKey: "tuning.modules.accelerator.label",
    hintKey: "tuning.modules.accelerator.hint",
    icon: Gauge,
    liveValueOf: (snapshot) => clamp01(snapshot.pedals.accel),
  },
  {
    id: "brake",
    labelKey: "tuning.modules.brake.label",
    hintKey: "tuning.modules.brake.hint",
    icon: Disc,
    liveValueOf: (snapshot) => clamp01(snapshot.pedals.brake),
  },
  {
    id: "clutch",
    labelKey: "tuning.modules.clutch.label",
    hintKey: "tuning.modules.clutch.hint",
    icon: Footprints,
    liveValueOf: (snapshot) =>
      snapshot.pedals.clutch === null
        ? null
        : clamp01(snapshot.pedals.clutch),
  },
  {
    id: "handbrake",
    labelKey: "tuning.modules.handbrake.label",
    hintKey: "tuning.modules.handbrake.hint",
    icon: Hand,
    liveValueOf: null,
  },
  {
    id: "shifter",
    labelKey: "tuning.modules.shifter.label",
    hintKey: "tuning.modules.shifter.hint",
    icon: ArrowUpDown,
    liveValueOf: null,
  },
  {
    id: "horn",
    labelKey: "tuning.modules.horn.label",
    hintKey: "tuning.modules.horn.hint",
    icon: Megaphone,
    liveValueOf: null,
  },
  {
    id: "buttons",
    labelKey: "tuning.modules.buttons.label",
    hintKey: "tuning.modules.buttons.hint",
    icon: LayoutGrid,
    liveValueOf: null,
  },
  {
    id: "encoders",
    labelKey: "tuning.modules.encoders.label",
    hintKey: "tuning.modules.encoders.hint",
    icon: RotateCw,
    liveValueOf: null,
  },
  {
    id: "led",
    labelKey: "tuning.modules.led.label",
    hintKey: "tuning.modules.led.hint",
    icon: Lightbulb,
    liveValueOf: null,
  },
];

export const MODULE_IDS: readonly ModuleId[] = MODULES.map((m) => m.id);

export function isModuleId(value: unknown): value is ModuleId {
  return (
    typeof value === "string" &&
    (MODULE_IDS as readonly string[]).includes(value)
  );
}

/**
 * Read per-module on/off state from a `get_settings` record. Returns only
 * the entries that are present and boolean; anything else is "Unknown"
 * (absent key), never guessed.
 *
 * Legacy alias (C-MOD-1): records written before the canonical rename
 * may still carry the old `"accel"` key — it maps to `"accelerator"`.
 * The canonical key wins when both are present.
 */
export function parseModuleStates(
  settings: Record<string, unknown>,
): Partial<Record<ModuleId, boolean>> {
  const raw = settings["modules"];
  if (raw === null || typeof raw !== "object" || Array.isArray(raw)) {
    return {};
  }
  const states: Partial<Record<ModuleId, boolean>> = {};
  const record = raw as Record<string, unknown>;
  if (typeof record["accel"] === "boolean") {
    states["accelerator"] = record["accel"];
  }
  for (const [key, value] of Object.entries(record)) {
    if (isModuleId(key) && typeof value === "boolean") {
      states[key] = value;
    }
  }
  return states;
}
