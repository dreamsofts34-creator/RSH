import { describe, it, expect } from "vitest";
import { validateCalibrationForm } from "./CalibrateStep";

/**
 * H10: empty fields are invalid, values must be 0..3300 mV, min < max,
 * and a pedal left entirely empty is omitted — never saved as 0/0.
 */
function form(overrides: {
  steering?: string;
  min?: Record<string, string>;
  max?: Record<string, string>;
}) {
  return {
    steeringCenterMv: overrides.steering ?? "1650",
    pedalMinMv: {
      accel: "0",
      brake: "0",
      clutch: "0",
      ...(overrides.min ?? {}),
    },
    pedalMaxMv: {
      accel: "3300",
      brake: "3300",
      clutch: "3300",
      ...(overrides.max ?? {}),
    },
  };
}

describe("validateCalibrationForm", () => {
  it("accepts a complete, in-range form", () => {
    const result = validateCalibrationForm(form({}));
    expect(result.errors).toEqual({});
    expect(result.omittedPedals).toEqual([]);
  });

  it("an empty steering field is invalid (never silently 0)", () => {
    const result = validateCalibrationForm(form({ steering: "" }));
    expect(result.errors["steering"]).toBe("empty");
  });

  it("an empty min with a filled max is invalid", () => {
    const result = validateCalibrationForm(
      form({ min: { accel: "" }, max: { accel: "3000" } }),
    );
    expect(result.errors["accel.min"]).toBe("empty");
    expect(result.omittedPedals).not.toContain("accel");
  });

  it("values outside 0..3300 mV are out of range", () => {
    const result = validateCalibrationForm(
      form({ min: { brake: "-5" }, max: { brake: "3301" } }),
    );
    expect(result.errors["brake.min"]).toBe("range");
    expect(result.errors["brake.max"]).toBe("range");
  });

  it("min must be below max", () => {
    const result = validateCalibrationForm(
      form({ min: { clutch: "2000" }, max: { clutch: "1000" } }),
    );
    expect(result.errors["clutch"]).toBe("minMax");
  });

  it("a pedal with both fields empty is omitted, not 0/0", () => {
    const result = validateCalibrationForm(
      form({ min: { clutch: "" }, max: { clutch: "" } }),
    );
    expect(result.errors).toEqual({});
    expect(result.omittedPedals).toEqual(["clutch"]);
  });

  it("a non-numeric value is invalid", () => {
    const result = validateCalibrationForm(form({ steering: "abc" }));
    expect(result.errors["steering"]).toBe("empty");
  });
});
