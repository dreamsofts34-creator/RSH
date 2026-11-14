import { describe, it, expect } from "vitest";
import { formatStat as formatMonitorStat } from "./PacketStats";
import { formatStat as formatDashboardStat } from "../../Dashboard/components/QuickStats";

/**
 * Nullable stat formatting (Phase F snapshot API): a `null` rate or
 * latency renders as "not measured", never as a fake 0. Packet loss
 * is not covered here — 0 % loss is a real value and is formatted by
 * the callers, not by `formatStat`.
 */
describe("formatStat with nullable stats", () => {
  it.each([
    ["monitor", formatMonitorStat],
    ["dashboard", formatDashboardStat],
  ])("%s: null renders as not measured", (_name, formatStat) => {
    expect(formatStat(null, "ms", "not measured")).toBe("not measured");
    expect(formatStat(null, "Hz", "not measured")).toBe("not measured");
  });

  it.each([
    ["monitor", formatMonitorStat],
    ["dashboard", formatDashboardStat],
  ])("%s: 0 still renders as not measured (never a real 0 ms)", (_name, formatStat) => {
    expect(formatStat(0, "ms", "not measured")).toBe("not measured");
  });

  it.each([
    ["monitor", formatMonitorStat],
    ["dashboard", formatDashboardStat],
  ])("%s: measured values render with units", (_name, formatStat) => {
    expect(formatStat(200, "Hz", "not measured")).toBe("200 Hz");
    expect(formatStat(3.4, "ms", "not measured")).toBe("3.4 ms");
    expect(formatStat(1.2, "ms", "not measured")).toBe("1.2 ms");
  });
});
