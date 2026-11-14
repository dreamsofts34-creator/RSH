import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import {
  ENCODER_PULSE_MS,
  EncoderPulseTimers,
  isEncoderButton,
} from "./encoderPulse";

/** Encoder buttons 65–72 are pulses (C5): pressed for one tick, then gone. */
describe("isEncoderButton", () => {
  it("is true for 65–72 and false for physical buttons 1–32", () => {
    for (let b = 65; b <= 72; b += 1) expect(isEncoderButton(b)).toBe(true);
    for (let b = 1; b <= 32; b += 1) expect(isEncoderButton(b)).toBe(false);
    expect(isEncoderButton(0)).toBe(false);
    expect(isEncoderButton(73)).toBe(false);
    expect(isEncoderButton(64.5)).toBe(false);
  });

  it("pulse window is 150–250 ms", () => {
    expect(ENCODER_PULSE_MS).toBeGreaterThanOrEqual(150);
    expect(ENCODER_PULSE_MS).toBeLessThanOrEqual(250);
  });
});

describe("EncoderPulseTimers", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("clears an encoder pulse after ~200 ms", () => {
    const timers = new EncoderPulseTimers();
    const clear = vi.fn();
    timers.arm(65, clear);
    expect(clear).not.toHaveBeenCalled();
    vi.advanceTimersByTime(199);
    expect(clear).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1);
    expect(clear).toHaveBeenCalledTimes(1);
  });

  it("never arms a physical button", () => {
    const timers = new EncoderPulseTimers();
    const clear = vi.fn();
    timers.arm(5, clear);
    vi.advanceTimersByTime(1000);
    expect(clear).not.toHaveBeenCalled();
  });

  it("re-arming restarts the timer", () => {
    const timers = new EncoderPulseTimers();
    const clear = vi.fn();
    timers.arm(66, clear);
    vi.advanceTimersByTime(150);
    timers.arm(66, clear);
    vi.advanceTimersByTime(150);
    expect(clear).not.toHaveBeenCalled();
    vi.advanceTimersByTime(50);
    expect(clear).toHaveBeenCalledTimes(1);
  });

  it("disarmAll cancels pending clears", () => {
    const timers = new EncoderPulseTimers();
    const clear = vi.fn();
    timers.arm(67, clear);
    timers.disarmAll();
    vi.advanceTimersByTime(1000);
    expect(clear).not.toHaveBeenCalled();
  });
});
