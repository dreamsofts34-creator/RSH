import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Gauge } from "lucide-react";
import {
  discoveredScreens,
  sortScreens,
  type ScreenDefinition,
} from "@/lib/screens";
import { getStartupNotice } from "@/lib/tauri";
import { ErrorBoundary } from "@/components/ErrorBoundary";
import { Button } from "@/components/ui/button";
import { Separator } from "@/components/ui/separator";
import { cn } from "@/lib/utils";

/**
 * App shell (UI plan P2.2): sidebar + content area.
 *
 * - Sidebar: brand, navigation for whatever screens the glob discovered
 *   (Wave 2 creates the screen files; with zero screens the sidebar shows
 *   only the brand). The theme switch lives ONLY in Settings →
 *   Appearance (UI plan golden rule 2; H3 removed the second switch
 *   that used to sit here and drifted from the persisted setting).
 * - Content: the active screen, or — when no screens exist yet — literal
 *   scaffold text via i18n (English fallback, never blank).
 * - A startup notice (corrupt-store repair / autostart-repair outcome)
 *   is shown once as a dismissible banner above the content.
 * - Navigation is real <button>s: Tab order is document order, and
 *   ArrowUp/ArrowDown move between nav items (keyboard-only flow, P10.2).
 */

export interface AppShellProps {
  /** Override the discovered screens (tests / embedding). Defaults to the glob. */
  screens?: ScreenDefinition[];
}

/** localStorage key for the last-open screen (M15). */
export const LAST_SCREEN_STORAGE_KEY = "rsh-shell.lastScreen";

function readLastScreen(paths: readonly string[]): string | null {
  try {
    const stored = window.localStorage.getItem(LAST_SCREEN_STORAGE_KEY);
    if (stored !== null && paths.includes(stored)) return stored;
  } catch {
    // Storage unavailable — fall through to the first screen.
  }
  return null;
}

export function AppShell({ screens }: AppShellProps) {
  const { t } = useTranslation();
  const navScreens = useMemo(
    () => sortScreens(screens ?? discoveredScreens),
    [screens],
  );
  const [activePath, setActivePath] = useState<string | null>(() =>
    readLastScreen(navScreens.map((s) => s.path)) ?? navScreens[0]?.path ?? null,
  );
  const navRefs = useRef<(HTMLButtonElement | null)[]>([]);
  const [startupNotice, setStartupNotice] = useState<string | null>(null);
  const [noticeDismissed, setNoticeDismissed] = useState(false);

  // Startup notice (H3): shown once, dismissible. Outside Tauri the
  // command fails and there is simply no banner — never an error.
  useEffect(() => {
    let alive = true;
    void getStartupNotice()
      .then((notice) => {
        if (alive && notice !== null) setStartupNotice(notice);
      })
      .catch(() => {});
    return () => {
      alive = false;
    };
  }, []);

  // If screens appear/change (e.g. a Wave-2 file lands), keep a valid selection.
  useEffect(() => {
    if (activePath === null && navScreens.length > 0) {
      setActivePath(navScreens[0].path);
    } else if (
      activePath !== null &&
      !navScreens.some((s) => s.path === activePath)
    ) {
      setActivePath(navScreens[0]?.path ?? null);
    }
  }, [navScreens, activePath]);

  // M15: remember the last screen across restarts.
  useEffect(() => {
    if (activePath === null) return;
    try {
      window.localStorage.setItem(LAST_SCREEN_STORAGE_KEY, activePath);
    } catch {
      // Storage unavailable — the shell still works for this session.
    }
  }, [activePath]);

  const activeScreen = navScreens.find((s) => s.path === activePath) ?? null;

  const focusNavItem = useCallback((index: number) => {
    const count = navRefs.current.length;
    if (count === 0) return;
    const next = (index + count) % count;
    navRefs.current[next]?.focus();
  }, []);

  const handleNavKeyDown = useCallback(
    (event: React.KeyboardEvent<HTMLButtonElement>, index: number) => {
      if (event.key === "ArrowDown") {
        event.preventDefault();
        focusNavItem(index + 1);
      } else if (event.key === "ArrowUp") {
        event.preventDefault();
        focusNavItem(index - 1);
      } else if (event.key === "Home") {
        // M15: Home/End jump to the first/last nav item.
        event.preventDefault();
        focusNavItem(0);
      } else if (event.key === "End") {
        event.preventDefault();
        focusNavItem(navRefs.current.length - 1);
      }
    },
    [focusNavItem],
  );

  return (
    <div className="flex h-screen w-full overflow-hidden bg-[var(--bg)] text-[var(--ink)]">
      <aside
        aria-label={t("app.title")}
        className="flex w-60 shrink-0 flex-col border-e border-[var(--line)] bg-[var(--card)]"
      >
        <div className="flex items-center gap-3 px-5 pb-4 pt-5">
          <span
            aria-hidden="true"
            className="flex h-10 w-10 items-center justify-center rounded-[12px] bg-[var(--accent)] text-[var(--on-accent)]"
          >
            <Gauge className="h-5 w-5" />
          </span>
          <div className="min-w-0">
            <p className="truncate text-base font-bold leading-tight">RSH</p>
            <p className="truncate text-xs text-[var(--muted)]">
              {t("app.tagline")}
            </p>
          </div>
        </div>

        <Separator />

        <nav
          aria-label={t("shell.nav", { defaultValue: "Screens" })}
          className="flex-1 overflow-y-auto px-3 py-3"
        >
          {navScreens.length === 0 ? null : (
            <ul className="space-y-1">
              {navScreens.map((screen, index) => {
                const Icon = screen.icon;
                const isActive = screen.path === activePath;
                return (
                  <li key={screen.path}>
                    <button
                      ref={(el) => {
                        navRefs.current[index] = el;
                      }}
                      type="button"
                      aria-current={isActive ? "page" : undefined}
                      onClick={() => setActivePath(screen.path)}
                      onKeyDown={(event) => handleNavKeyDown(event, index)}
                      className={cn(
                        "flex w-full items-center gap-3 rounded-lg px-3 py-2.5 text-left text-sm transition-colors",
                        isActive
                          ? "bg-[var(--nav-active-bg)] font-semibold text-[var(--nav-active-fg)]"
                          : "text-[var(--muted)] hover:bg-[var(--nav-hover-bg)] hover:text-[var(--ink)]",
                      )}
                    >
                      <span
                        aria-hidden="true"
                        className={cn(
                          "h-2 w-2 shrink-0 rounded-full",
                          isActive
                            ? "bg-[var(--accent)]"
                            : "bg-[var(--nav-dot)]",
                        )}
                      />
                      <Icon className="h-4 w-4 shrink-0" aria-hidden="true" />
                      <span className="truncate">
                        {t(screen.labelKey, { defaultValue: screen.label })}
                      </span>
                    </button>
                  </li>
                );
              })}
            </ul>
          )}
        </nav>
      </aside>

      <main className="min-w-0 flex-1 overflow-y-auto p-6">
        {startupNotice !== null && !noticeDismissed ? (
          <div
            role="status"
            className="mb-4 flex items-start justify-between gap-3 rounded-[10px] border border-[var(--line)] bg-[var(--banner)] p-3 text-sm"
          >
            <span>{startupNotice}</span>
            <Button
              type="button"
              size="sm"
              variant="outline"
              onClick={() => setNoticeDismissed(true)}
            >
              {t("shell.dismissNotice", { defaultValue: "Dismiss" })}
            </Button>
          </div>
        ) : null}
        {activeScreen ? (
          <ErrorBoundary
            key={activeScreen.path}
            label={t(activeScreen.labelKey, {
              defaultValue: activeScreen.label,
            })}
          >
            <activeScreen.component />
          </ErrorBoundary>
        ) : (
          <section
            aria-label={t("shell.emptyTitle")}
            className="flex h-full flex-col items-start justify-center gap-2"
          >
            <h1 className="text-[26px] font-bold leading-tight">
              {t("shell.emptyTitle")}
            </h1>
            <p className="max-w-md text-sm text-[var(--muted)]">
              {t("shell.emptyBody")}
            </p>
          </section>
        )}
      </main>
    </div>
  );
}
