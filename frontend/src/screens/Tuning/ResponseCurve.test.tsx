import { describe, it, expect } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import { ResponseCurve } from "./ResponseCurve";

/**
 * The chart renders from a small static dataset. jsdom has no canvas 2D
 * context, so uPlot construction throws here and the SVG fallback path
 * is what these tests exercise — the same points, the same live marker.
 * (The uPlot path runs in the real WebView2 runtime.)
 */

describe("ResponseCurve", () => {
  it("renders the curve from the deadzone/curve values (SVG fallback)", async () => {
    render(
      <ResponseCurve
        deadzone={0}
        curve={1}
        liveInput={null}
        ariaLabel="Response curve"
      />,
    );
    expect(
      screen.getByRole("img", { name: "Response curve" }),
    ).toBeInTheDocument();
    // The uPlot chunk loads (and fails in jsdom) asynchronously; the
    // SVG fallback appears once that resolves.
    await waitFor(() =>
      expect(document.querySelector("polyline")).not.toBeNull(),
    );
    const polyline = document.querySelector("polyline");
    expect(polyline).not.toBeNull();
    const points = polyline?.getAttribute("points")?.trim().split(/\s+/) ?? [];
    // 61 sampled points from computeCurvePoints.
    expect(points.length).toBe(61);
    expect(points[0]).toBe("0.00,100.00");
    expect(points[points.length - 1]).toBe("100.00,0.00");
  });

  it("shapes the curve with deadzone and exponent", async () => {
    const { rerender } = render(
      <ResponseCurve
        deadzone={0}
        curve={1}
        liveInput={null}
        ariaLabel="Response curve"
      />,
    );
    await waitFor(() =>
      expect(document.querySelector("polyline")).not.toBeNull(),
    );
    const linear = document.querySelector("polyline")?.getAttribute("points");
    rerender(
      <ResponseCurve
        deadzone={10}
        curve={2}
        liveInput={null}
        ariaLabel="Response curve"
      />,
    );
    await waitFor(() => {
      const shaped = document
        .querySelector("polyline")
        ?.getAttribute("points");
      expect(shaped).not.toBe(linear);
    });
    const shaped = document.querySelector("polyline")?.getAttribute("points");
    // With a 10% deadzone the first 10% of the curve is flat at y=0 (100 in SVG).
    const shapedPoints = shaped?.trim().split(/\s+/) ?? [];
    expect(shapedPoints[3]).toBe("5.00,100.00");
  });

  it("shows the live-input marker at the shaped position", () => {
    render(
      <ResponseCurve
        deadzone={0}
        curve={1}
        liveInput={0.5}
        ariaLabel="Response curve"
      />,
    );
    const chart = screen.getByRole("img", { name: "Response curve" });
    // The marker dot is the absolutely positioned span inside the chart.
    const dot = chart.querySelector("span.absolute.h-3\\.5");
    expect(dot).not.toBeNull();
    expect(dot?.getAttribute("style")).toContain("left: 50%");
    expect(dot?.getAttribute("style")).toContain("bottom: 50%");
  });

  it("renders no marker when there is no live input", () => {
    render(
      <ResponseCurve
        deadzone={0}
        curve={1}
        liveInput={null}
        ariaLabel="Response curve"
      />,
    );
    const chart = screen.getByRole("img", { name: "Response curve" });
    expect(chart.querySelector("span.absolute.h-3\\.5")).toBeNull();
  });

  it("labels the input axis 0 / 50 / 100 %", () => {
    render(
      <ResponseCurve
        deadzone={0}
        curve={1}
        liveInput={null}
        ariaLabel="Response curve"
      />,
    );
    expect(screen.getByText("0%")).toBeInTheDocument();
    expect(screen.getByText("50%")).toBeInTheDocument();
    expect(screen.getByText("100%")).toBeInTheDocument();
  });
});
