import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Settings as SettingsIcon } from "lucide-react";
import { changeLanguage, type SupportedLanguage } from "@/lib/i18n";
import { setTheme, type ThemeId } from "@/lib/theme";
import {
  getSettings,
  setSettings,
  TauriCommandError,
} from "@/lib/tauri";
import { AppearanceSection } from "./AppearanceSection";
import { DataSection } from "./DataSection";
import { ModuleToggles } from "./ModuleToggles";
import { PinMapEditor } from "./PinMapEditor";
import { SystemSection } from "./SystemSection";
import { TransportSection } from "./TransportSection";
import {
  DEFAULT_SETTINGS,
  fullSettingsPayload,
  parseSettings,
  st,
  type ModuleId,
  type SettingsState,
} from "./settings";

/**
 * Settings screen (UI plan P9) — route `settings`, last in the sidebar.
 *
 * Sections: Modules toggles, GPIO pin map editor, Appearance (theme +
 * language — the only place these switches live), Transport, Data,
 * System. Every control reads/writes real state through the `tauri.ts`
 * wrappers; until the Rust commands exist each section shows its honest
 * unavailable state instead of fake data.
 */

export const path = "settings";
export const label = "Settings";
export const icon = SettingsIcon;
export const component = SettingsScreen;

type LoadStatus = "loading" | "ready" | "unavailable" | "error";

export function SettingsScreen() {
  const { t } = useTranslation();
  const [settings, setSettingsState] = useState<SettingsState | null>(null);
  const [loadStatus, setLoadStatus] = useState<LoadStatus>("loading");
  const [status, setStatus] = useState<string | null>(null);
  /**
   * The raw record `get_settings` returned (C1): saves send the FULL
   * record with our fields overlaid, so fields this screen does not
   * own are never dropped.
   */
  const rawRecordRef = useRef<Record<string, unknown> | null>(null);
  /** In-flight save guard (M10): one save at a time; a newer patch
   *  queued while one is in flight runs right after it, never lost. */
  const savingRef = useRef(false);
  const pendingPatchRef = useRef<Partial<SettingsState> | null>(null);
  const pendingResolversRef = useRef<((ok: boolean) => void)[]>([]);
  /** Mirror of `settings` for the save path (avoids stale closures). */
  const settingsRef = useRef<SettingsState | null>(null);
  useEffect(() => {
    settingsRef.current = settings;
  }, [settings]);

  useEffect(() => {
    let alive = true;
    void getSettings()
      .then((raw) => {
        if (!alive) return;
        rawRecordRef.current =
          raw !== null && typeof raw === "object" && !Array.isArray(raw)
            ? (raw as Record<string, unknown>)
            : null;
        setSettingsState(parseSettings(raw));
        setLoadStatus("ready");
      })
      .catch((error: unknown) => {
        if (!alive) return;
        setLoadStatus(
          error instanceof TauriCommandError && error.notImplemented
            ? "unavailable"
            : "error",
        );
      });
    return () => {
      alive = false;
    };
  }, []);

  /**
   * Persist a patch via `set_settings` (full record, C-SET-1); revert
   * on failure, never lie. Returns true when the save landed. When a
   * save is already in flight the patch is queued and the returned
   * promise resolves with the queued save's outcome.
   */
  async function saveSettings(patch: Partial<SettingsState>): Promise<boolean> {
    const current = settingsRef.current;
    if (current === null) return false;
    // A save is already in flight: merge this patch into the queued
    // one and resolve when the follow-up save finishes.
    if (savingRef.current) {
      pendingPatchRef.current = {
        ...pendingPatchRef.current,
        ...patch,
      };
      return new Promise<boolean>((resolve) => {
        pendingResolversRef.current.push(resolve);
      });
    }
    const next: SettingsState = { ...current, ...patch };
    const previous = current;
    settingsRef.current = next;
    setSettingsState(next);
    savingRef.current = true;
    let ok: boolean;
    try {
      const saved = await setSettings({
        settings: fullSettingsPayload(rawRecordRef.current, next),
      });
      if (
        saved !== null &&
        typeof saved === "object" &&
        !Array.isArray(saved)
      ) {
        rawRecordRef.current = saved as Record<string, unknown>;
      }
      setStatus(st(t, "settings.saved", "Saved."));
      ok = true;
    } catch {
      settingsRef.current = previous;
      setSettingsState(previous);
      setStatus(st(t, "settings.saveFailed", "Could not save settings."));
      ok = false;
    } finally {
      savingRef.current = false;
      // A patch arrived while saving: run it now (it re-queues itself
      // if another save started meanwhile).
      const pending = pendingPatchRef.current;
      const resolvers = pendingResolversRef.current;
      pendingPatchRef.current = null;
      pendingResolversRef.current = [];
      if (pending !== null) {
        const pendingOk = await saveSettings(pending);
        for (const resolve of resolvers) resolve(pendingOk);
      }
    }
    return ok;
  }

  async function handleModuleToggle(id: ModuleId, enabled: boolean) {
    const current = settingsRef.current;
    if (current === null) return;
    await saveSettings({
      modules: { ...current.modules, [id]: enabled },
    });
  }

  async function handleThemeChange(theme: ThemeId) {
    const current = settingsRef.current;
    if (current === null) return;
    const previous = current.theme;
    // Apply immediately for this session, then persist via the backend.
    setTheme(theme);
    // M10: if the persist fails, revert the applied theme too —
    // localStorage and the document must match the persisted state.
    if (!(await saveSettings({ theme }))) setTheme(previous);
  }

  async function handleLanguageChange(language: SupportedLanguage) {
    const current = settingsRef.current;
    if (current === null) return;
    const previous = current.language;
    // Runtime switch, no restart — then persist via the backend.
    await changeLanguage(language);
    // M10: on persist failure revert the language (localStorage and
    // document lang/dir) so the UI matches the persisted state.
    if (!(await saveSettings({ language }))) await changeLanguage(previous);
  }

  const effective = settings ?? DEFAULT_SETTINGS;
  const controlsDisabled = loadStatus !== "ready";

  return (
    <section
      aria-label={st(t, "settings.title", "Settings")}
      className="mx-auto w-full max-w-3xl space-y-6"
    >
      <header className="space-y-1">
        <h1 className="text-[26px] font-bold leading-tight">
          {st(t, "settings.title", "Settings")}
        </h1>
        <p className="text-sm text-[var(--muted)]">
          {st(
            t,
            "settings.subtitle",
            "Modules, pins, appearance, transport, data and system.",
          )}
        </p>
      </header>

      {loadStatus === "loading" ? (
        <p className="text-sm text-[var(--muted)]">
          {st(t, "settings.loading", "Loading settings…")}
        </p>
      ) : null}
      {loadStatus === "unavailable" ? (
        <p
          role="alert"
          className="rounded-[10px] border border-[var(--line)] bg-[var(--banner)] p-3 text-sm"
        >
          {st(
            t,
            "settings.unavailable",
            "Settings are not available yet — the backend command is not implemented.",
          )}
        </p>
      ) : null}
      {loadStatus === "error" ? (
        <p role="alert" className="text-sm text-[var(--error-text)]">
          {st(t, "settings.loadFailed", "Could not load settings.")}
        </p>
      ) : null}

      {status !== null ? (
        <p aria-live="polite" className="text-sm text-[var(--muted)]">
          {status}
        </p>
      ) : null}

      <ModuleToggles
        modules={effective.modules}
        disabled={controlsDisabled}
        onToggle={(id, enabled) => void handleModuleToggle(id, enabled)}
      />
      <PinMapEditor />
      <AppearanceSection
        theme={effective.theme}
        language={effective.language}
        disabled={controlsDisabled}
        onThemeChange={(theme) => void handleThemeChange(theme)}
        onLanguageChange={(language) => void handleLanguageChange(language)}
      />
      <TransportSection />
      <DataSection />
      <SystemSection />
    </section>
  );
}

export default SettingsScreen;
