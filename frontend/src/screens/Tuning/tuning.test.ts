import { describe, it, expect } from "vitest";
import {
  applyResponse,
  clampEndpoints,
  clampNumber,
  computeCurvePoints,
  defaultTuning,
  normalizeTuning,
  sameTuning,
  FULL_SCALE_MV,
  MIN_SPAN_MV,
} from "./tuning";
import { isModuleId, MODULES, parseModuleStates } from "./modules";

describe("tuning value model", () => {
  it("defaults are linear, wide open, no deadzone or smoothing", () => {
    const defaults = defaultTuning("steering");
    expect(defaults).toEqual({
      module: "steering",
      deadzone: 0,
      curve: 1,
      minMv: 0,
      maxMv: FULL_SCALE_MV,
      smoothing: 0,
    });
  });

  it("sameTuning compares every field", () => {
    const a = defaultTuning("brake");
    expect(sameTuning(a, { ...a })).toBe(true);
    expect(sameTuning(a, { ...a, deadzone: 1 })).toBe(false);
    expect(sameTuning(a, { ...a, module: "accelerator" })).toBe(false);
  });

  it("clampNumber clamps and treats non-finite as the minimum", () => {
    expect(clampNumber(5, 0, 10)).toBe(5);
    expect(clampNumber(-3, 0, 10)).toBe(0);
    expect(clampNumber(99, 0, 10)).toBe(10);
    expect(clampNumber(Number.NaN, 0, 10)).toBe(0);
  });

  it("endpoint guard leaves a legal span alone", () => {
    expect(clampEndpoints(0, 3300, "min")).toEqual({
      minMv: 0,
      maxMv: 3300,
      clamped: false,
    });
    expect(clampEndpoints(0, 660, "max")).toEqual({
      minMv: 0,
      maxMv: 660,
      clamped: false,
    });
  });

  it("endpoint guard clamps the moved minimum back to max - 660 mV", () => {
    expect(clampEndpoints(3000, 3300, "min")).toEqual({
      minMv: 3300 - MIN_SPAN_MV,
      maxMv: 3300,
      clamped: true,
    });
  });

  it("endpoint guard clamps the moved maximum up to min + 660 mV", () => {
    expect(clampEndpoints(0, 500, "max")).toEqual({
      minMv: 0,
      maxMv: MIN_SPAN_MV,
      clamped: true,
    });
  });

  it("endpoint guard never pushes endpoints outside 0..3300 mV", () => {
    expect(clampEndpoints(3300, 3300, "min")).toEqual({
      minMv: 3300 - MIN_SPAN_MV,
      maxMv: 3300,
      clamped: true,
    });
    expect(clampEndpoints(0, 0, "max")).toEqual({
      minMv: 0,
      maxMv: MIN_SPAN_MV,
      clamped: true,
    });
  });

  it("normalizeTuning clamps out-of-range backend values", () => {
    const normalised = normalizeTuning({
      module: "accelerator",
      deadzone: 99,
      curve: 9,
      minMv: -50,
      maxMv: 99999,
      smoothing: -1,
    });
    expect(normalised).toEqual({
      module: "accelerator",
      deadzone: 30,
      curve: 2,
      minMv: 0,
      maxMv: 3300,
      smoothing: 0,
    });
  });

  it("normalizeTuning repairs a violating endpoint span from the backend", () => {
    const normalised = normalizeTuning({
      module: "brake",
      deadzone: 0,
      curve: 1,
      minMv: 3000,
      maxMv: 3300,
      smoothing: 0,
    });
    expect(normalised.minMv).toBe(3300 - MIN_SPAN_MV);
    expect(normalised.maxMv).toBe(3300);
  });

  it("normalizeTuning falls back to defaults for non-finite values", () => {
    const normalised = normalizeTuning({
      module: "led",
      deadzone: Number.NaN,
      curve: Number.POSITIVE_INFINITY,
      minMv: 100,
      maxMv: 2000,
      smoothing: 10,
    });
    expect(normalised.deadzone).toBe(0);
    expect(normalised.curve).toBe(1);
    expect(normalised.minMv).toBe(100);
    expect(normalised.maxMv).toBe(2000);
  });

  it("applyResponse is identity for a linear curve with no deadzone", () => {
    expect(applyResponse(0, 0, 1)).toBe(0);
    expect(applyResponse(0.5, 0, 1)).toBeCloseTo(0.5);
    expect(applyResponse(1, 0, 1)).toBe(1);
  });

  it("applyResponse zeroes input inside the deadzone and rescales above it", () => {
    expect(applyResponse(0.05, 10, 1)).toBe(0);
    expect(applyResponse(0.1, 10, 1)).toBe(0);
    // (0.55 - 0.10) / (1 - 0.10) = 0.5
    expect(applyResponse(0.55, 10, 1)).toBeCloseTo(0.5);
  });

  it("applyResponse applies the curve exponent after the deadzone", () => {
    // normalised 0.5, squared -> 0.25
    expect(applyResponse(0.55, 10, 2)).toBeCloseTo(0.25);
    // normalised 0.5, square root -> ~0.707
    expect(applyResponse(0.55, 10, 0.5)).toBeCloseTo(Math.SQRT1_2);
  });

  it("computeCurvePoints starts at (0,0), ends at (1,1) and is monotonic", () => {
    const points = computeCurvePoints(5, 1.5);
    expect(points.length).toBe(61);
    expect(points[0]).toEqual({ x: 0, y: 0 });
    const last = points[points.length - 1];
    expect(last?.x).toBe(1);
    expect(last?.y).toBeCloseTo(1);
    for (let i = 1; i < points.length; i++) {
      expect(points[i]?.y).toBeGreaterThanOrEqual(points[i - 1]?.y ?? 0);
    }
  });
});

describe("module registry", () => {
  it("has exactly the 10 approved modules, in order", () => {
    expect(MODULES.map((m) => m.id)).toEqual([
      "steering",
      "accelerator",
      "brake",
      "clutch",
      "handbrake",
      "shifter",
      "horn",
      "buttons",
      "encoders",
      "led",
    ]);
  });

  it("only steering and the pedals have a live snapshot channel", () => {
    const live = MODULES.filter((m) => m.liveValueOf !== null).map(
      (m) => m.id,
    );
    expect(live).toEqual(["steering", "accelerator", "brake", "clutch"]);
  });

  it("steering live value maps -450..450 deg to 0..1", () => {
    const steering = MODULES[0];
    const snapshot = (deg: number) => ({
      steeringDeg: deg,
      pedals: { accel: 0, brake: 0, clutch: null },
      connection: "connected" as const,
      stats: {
        loopHz: 200,
        lossPct: 0,
        jitterMsP99: 0,
        outputLatencyMs: 0,
        e2eMs: 0,
      },
      activeProfileId: null,
      seq: 1,
      seqGap: 0,
      txFail: 0,
    });
    expect(steering?.liveValueOf?.(snapshot(-450))).toBe(0);
    expect(steering?.liveValueOf?.(snapshot(0))).toBe(0.5);
    expect(steering?.liveValueOf?.(snapshot(450))).toBe(1);
  });

  it("clutch live value is null when the protocol has no clutch channel", () => {
    const clutch = MODULES[3];
    const snapshot = {
      steeringDeg: 0,
      pedals: { accel: 0, brake: 0, clutch: null },
      connection: "connected" as const,
      stats: {
        loopHz: 200,
        lossPct: 0,
        jitterMsP99: 0,
        outputLatencyMs: 0,
        e2eMs: 0,
      },
      activeProfileId: null,
      seq: 1,
      seqGap: 0,
      txFail: 0,
    };
    expect(clutch?.liveValueOf?.(snapshot)).toBeNull();
  });

  it("isModuleId accepts only the 10 known ids", () => {
    expect(isModuleId("steering")).toBe(true);
    expect(isModuleId("led")).toBe(true);
    expect(isModuleId("wheel")).toBe(false);
    expect(isModuleId("")).toBe(false);
    expect(isModuleId(undefined)).toBe(false);
  });

  it("parseModuleStates reads only boolean entries under settings.modules", () => {
    expect(
      parseModuleStates({
        modules: { steering: true, brake: false, bogus: true, led: "yes" },
      }),
    ).toEqual({ steering: true, brake: false });
    expect(parseModuleStates({})).toEqual({});
    expect(parseModuleStates({ modules: "steering" })).toEqual({});
    expect(parseModuleStates({ modules: null })).toEqual({});
    expect(parseModuleStates({ modules: ["steering"] })).toEqual({});
  });
});

describe("tuning extension fields (bite point, gate, horn, LED)", () => {
  it("sameTuning compares extension fields", () => {
    const a = { ...defaultTuning("brake"), bitePoint: 0.35 };
    expect(sameTuning(a, { ...a })).toBe(true);
    expect(sameTuning(a, { ...a, bitePoint: 0.4 })).toBe(false);
    expect(sameTuning(a, { ...a, bitePoint: undefined })).toBe(false);
    const led = { ...defaultTuning("led"), ledBrightness: 80, ledPattern: "rainbow" as const };
    expect(sameTuning(led, { ...led })).toBe(true);
    expect(sameTuning(led, { ...led, ledPattern: "blink" as const })).toBe(false);
  });

  it("normalizeTuning keeps valid extension values", () => {
    const normalised = normalizeTuning({
      ...defaultTuning("clutch"),
      bitePoint: 0.35,
      shifterGate: "h-pattern",
      hornButton: 3,
      ledBrightness: 80,
      ledPattern: "rainbow",
    });
    expect(normalised.bitePoint).toBe(0.35);
    expect(normalised.shifterGate).toBe("h-pattern");
    expect(normalised.hornButton).toBe(3);
    expect(normalised.ledBrightness).toBe(80);
    expect(normalised.ledPattern).toBe("rainbow");
  });

  it("normalizeTuning drops invalid extension values instead of keeping them", () => {
    const normalised = normalizeTuning({
      ...defaultTuning("shifter"),
      bitePoint: 2,
      shifterGate: "sideways" as unknown as "sequential",
      hornButton: 40,
      ledBrightness: 250,
      ledPattern: "strobe" as unknown as "solid",
    });
    // Out-of-range numbers are clamped (like the base fields), enums and
    // out-of-range button numbers are dropped.
    expect(normalised.bitePoint).toBe(1);
    expect(normalised.shifterGate).toBeUndefined();
    expect(normalised.hornButton).toBeUndefined();
    // Out-of-range brightness is clamped, not dropped.
    expect(normalised.ledBrightness).toBe(100);
    expect(normalised.ledPattern).toBeUndefined();
  });
});

describe("module id parity with Settings (C-MOD-1 / C2)", () => {
  it("Tuning and Settings agree on the canonical module ids", async () => {
    const { MODULE_IDS: tuningIds } = await import("./modules");
    const { MODULE_IDS: settingsIds } = await import(
      "../Settings/settings"
    );
    expect([...tuningIds]).toEqual([...settingsIds]);
    expect(tuningIds).toContain("accelerator");
    expect(tuningIds).not.toContain("accel");
  });

  it("parseModuleStates maps the legacy 'accel' key to 'accelerator'", async () => {
    const { parseModuleStates } = await import("./modules");
    expect(
      parseModuleStates({ modules: { accel: false } }),
    ).toEqual({ accelerator: false });
    expect(
      parseModuleStates({
        modules: { accel: false, accelerator: true },
      }),
    ).toEqual({ accelerator: true });
  });
});
