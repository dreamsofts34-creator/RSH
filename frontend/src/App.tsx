import { AppShell } from "@/components/AppShell";
import { ErrorBoundary } from "@/components/ErrorBoundary";

/**
 * RSH frontend root. The shell owns navigation + theming; screens are
 * discovered by the glob in src/lib/screens.ts (Wave 2 creates them).
 * No business logic lives in the frontend.
 *
 * The outer error boundary (C6) is the last line of defence against a
 * blank window: if the shell itself crashes, the user still gets a
 * fallback with a reload button instead of an empty window.
 */
export default function App() {
  return (
    <ErrorBoundary label="the app">
      <AppShell />
    </ErrorBoundary>
  );
}
