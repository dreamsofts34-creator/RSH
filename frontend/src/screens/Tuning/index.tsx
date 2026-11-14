import { SlidersHorizontal } from "lucide-react";
import { TuningScreen } from "./TuningScreen";

/**
 * Tuning screen module (UI plan P4). Discovered by the glob in
 * `src/lib/screens.ts` — no registry edit is needed.
 */
export const path = "tuning";
export const label = "Tuning";
export const icon = SlidersHorizontal;
export const component = TuningScreen;

export { TuningScreen };
