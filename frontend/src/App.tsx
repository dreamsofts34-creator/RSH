import { AppShell } from "@/components/AppShell";

/**
 * RSH frontend root. The shell owns navigation + theming; screens are
 * discovered by the glob in src/lib/screens.ts (Wave 2 creates them).
 * No business logic lives in the frontend.
 */
export default function App() {
  return <AppShell />;
}
