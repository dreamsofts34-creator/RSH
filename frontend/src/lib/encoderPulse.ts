/**
 * Encoder virtual buttons 65–72 are pulses, not holds (C5).
 *
 * The backend reports an encoder step as a button that is pressed for
 * exactly one tick: the `rsh://button-press` event carries
 * `pressed: true` and the next snapshot no longer lists the button —
 * but no `pressed: false` event ever arrives for it. Without a local
 * auto-clear the highlight would stick forever. Physical buttons
 * (1–32) DO get a real release event, so they are never auto-cleared:
 * clearing one would hide a genuinely held button.
 *
 * The pulse window (~200 ms) is long enough to see at 30 Hz and short
 * enough that a fast encoder spin never leaves a trail of stuck
 * highlights.
 */

/** First/last encoder virtual button (four encoders × CW/CCW). */
export const ENCODER_BUTTON_MIN = 65;
export const ENCODER_BUTTON_MAX = 72;

/** How long an encoder pulse highlight stays visible (ms). */
export const ENCODER_PULSE_MS = 200;

/** True for encoder virtual buttons 65–72 (pulse); false for 1–32 (hold). */
export function isEncoderButton(button: number): boolean {
  return (
    Number.isInteger(button) &&
    button >= ENCODER_BUTTON_MIN &&
    button <= ENCODER_BUTTON_MAX
  );
}

/**
 * Auto-clear timers for encoder pulses. One instance per component:
 * call `arm(button, clear)` when a pulse press arrives; the clear
 * callback fires ~200 ms later unless `disarmAll()` runs first
 * (unmount). Re-arming the same button restarts its timer.
 */
export class EncoderPulseTimers {
  private readonly timers = new Map<number, number>();

  arm(button: number, clear: () => void): void {
    if (!isEncoderButton(button)) return;
    this.disarm(button);
    this.timers.set(
      button,
      window.setTimeout(() => {
        this.timers.delete(button);
        clear();
      }, ENCODER_PULSE_MS),
    );
  }

  disarm(button: number): void {
    const timer = this.timers.get(button);
    if (timer !== undefined) {
      window.clearTimeout(timer);
      this.timers.delete(button);
    }
  }

  disarmAll(): void {
    for (const timer of this.timers.values()) window.clearTimeout(timer);
    this.timers.clear();
  }
}
