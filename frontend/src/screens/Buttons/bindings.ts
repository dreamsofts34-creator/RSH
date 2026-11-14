/**
 * Button bindings (UI plan P6.1).
 *
 * Bindings live in the device config (`get_config`): `config.buttons` is a
 * map from physical button number (as a string) to an action id, e.g.
 * `{ "1": "horn", "5": "shifter_up" }`. There is no dedicated
 * button-binding command in `src/lib/tauri.ts` yet, so bindings are read
 * and written through the config staging flow
 * (`stage_config` -> `commit_config` -> `confirm_config`).
 *
 * Physical buttons are 1–32 (wire `buttons` u32, protocol v2.1 §1.1).
 * Encoder virtual buttons are 65–72: four encoders × clockwise /
 * counter-clockwise (software plan SW-R3: encoder→button mapping).
 */

export const PHYSICAL_BUTTONS: readonly number[] = Array.from(
  { length: 32 },
  (_, i) => i + 1,
);

/** Encoder virtual buttons: 65–72 (encoder 1–4, CW then CCW each). */
export const ENCODER_BUTTONS: readonly {
  button: number;
  encoder: number;
  direction: "cw" | "ccw";
}[] = [
  { button: 65, encoder: 1, direction: "cw" },
  { button: 66, encoder: 1, direction: "ccw" },
  { button: 67, encoder: 2, direction: "cw" },
  { button: 68, encoder: 2, direction: "ccw" },
  { button: 69, encoder: 3, direction: "cw" },
  { button: 70, encoder: 3, direction: "ccw" },
  { button: 71, encoder: 4, direction: "cw" },
  { button: 72, encoder: 4, direction: "ccw" },
];

/** Actions a button can trigger (feature inventory: horn, shifter, gears, handbrake). */
export const ASSIGNABLE_ACTIONS: readonly string[] = [
  "horn",
  "shifter_up",
  "shifter_down",
  "gear_1",
  "gear_2",
  "gear_3",
  "gear_4",
  "gear_5",
  "gear_6",
  "handbrake",
];

/** Button number -> action id. Absent = unbound (shown honestly, never invented). */
export type ButtonBindings = Record<number, string>;

/**
 * Parse `config.buttons` defensively: only integer keys with string values
 * count; anything else is ignored (a corrupt entry must not invent a
 * binding or crash the screen).
 */
export function parseBindings(config: Record<string, unknown>): ButtonBindings {
  const raw = config["buttons"];
  if (raw === null || typeof raw !== "object" || Array.isArray(raw)) {
    return {};
  }
  const bindings: ButtonBindings = {};
  for (const [key, value] of Object.entries(raw as Record<string, unknown>)) {
    const button = Number(key);
    if (
      Number.isInteger(button) &&
      button >= 1 &&
      typeof value === "string" &&
      value.length > 0
    ) {
      bindings[button] = value;
    }
  }
  return bindings;
}

/** Return a copy of `config` with one binding replaced (or removed when unbound). */
export function withBinding(
  config: Record<string, unknown>,
  button: number,
  action: string | null,
): Record<string, unknown> {
  const current = parseBindings(config);
  if (action === null) {
    delete current[button];
  } else {
    current[button] = action;
  }
  const next: Record<string, unknown> = { ...config };
  const asStrings: Record<string, string> = {};
  for (const [key, value] of Object.entries(current)) {
    asStrings[key] = value;
  }
  next["buttons"] = asStrings;
  return next;
}
