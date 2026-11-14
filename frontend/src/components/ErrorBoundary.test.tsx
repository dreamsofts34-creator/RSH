import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { ErrorBoundary } from "./ErrorBoundary";

/** C6: a crashing screen renders a fallback with a reload action,
 *  never a blank window. */
describe("ErrorBoundary", () => {
  it("renders children when nothing crashes", () => {
    render(
      <ErrorBoundary label="Dashboard">
        <p>Dashboard content</p>
      </ErrorBoundary>,
    );
    expect(screen.getByText("Dashboard content")).toBeInTheDocument();
  });

  it("renders the fallback with the error and a reload button on crash", () => {
    const onReload = vi.fn();
    const Thrower = () => {
      throw new Error("boom in render");
    };
    // React logs the error to the console; silence it for this test.
    const consoleSpy = vi
      .spyOn(console, "error")
      .mockImplementation(() => {});
    try {
      render(
        <ErrorBoundary label="Dashboard" onReload={onReload}>
          <Thrower />
        </ErrorBoundary>,
      );
    } finally {
      consoleSpy.mockRestore();
    }
    expect(screen.getByRole("alert")).toBeInTheDocument();
    expect(screen.getByText(/Something went wrong/)).toBeInTheDocument();
    expect(screen.getByText("boom in render")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Reload" }));
    expect(onReload).toHaveBeenCalledTimes(1);
  });
});
