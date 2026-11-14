import { Component, type ReactNode } from "react";
import i18n from "@/lib/i18n";

/**
 * Error boundary (C6 — the C# blank-window lesson).
 *
 * A render crash in any screen used to blank the whole window with no
 * way back. The boundary catches the crash, renders a fallback with
 * real text and a reload action, and never swallows the error silently
 * (it is logged to the console for diagnostics). The shell wraps the
 * whole app in one boundary and each active screen in another, keyed
 * by screen path so navigating away resets the crashed screen.
 */

export interface ErrorBoundaryProps {
  children: ReactNode;
  /** Short label for what crashed ("Dashboard", "the app") — shown in the fallback. */
  label: string;
  /** Called instead of reloading the page (tests inject a stub). */
  onReload?: () => void;
}

interface ErrorBoundaryState {
  error: Error | null;
}

export class ErrorBoundary extends Component<
  ErrorBoundaryProps,
  ErrorBoundaryState
> {
  state: ErrorBoundaryState = { error: null };

  static getDerivedStateFromError(error: Error): ErrorBoundaryState {
    return { error };
  }

  componentDidCatch(error: Error): void {
    console.error(`[ErrorBoundary] ${this.props.label} crashed:`, error);
  }

  private handleReload = (): void => {
    if (this.props.onReload) {
      this.props.onReload();
      return;
    }
    window.location.reload();
  };

  render(): ReactNode {
    if (this.state.error !== null) {
      const t = (key: string, fallback: string): string =>
        i18n.t(key, { defaultValue: fallback });
      return (
        <div
          role="alert"
          className="mx-auto w-full max-w-xl space-y-3 rounded-2xl border border-[var(--line)] bg-[var(--card)] p-6"
        >
          <h2 className="text-lg font-bold text-[var(--ink)]">
            {t("errorBoundary.title", "Something went wrong")} in{" "}
            {this.props.label}.
          </h2>
          <p className="text-sm text-[var(--muted)]">
            {t(
              "errorBoundary.body",
              "This screen crashed, but the rest of the app is safe. Reload to try again — your profiles and settings were not touched.",
            )}
          </p>
          <p className="rounded-[10px] bg-[var(--banner)] px-3 py-2 font-mono text-xs text-[var(--ink)]">
            {this.state.error.message}
          </p>
          <button
            type="button"
            onClick={this.handleReload}
            className="rounded-[10px] bg-[var(--accent)] px-4 py-2 text-sm font-semibold text-[var(--on-accent)]"
          >
            {t("errorBoundary.reload", "Reload")}
          </button>
        </div>
      );
    }
    return this.props.children;
  }
}
