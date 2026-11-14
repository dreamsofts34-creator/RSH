import { useEffect, useState } from "react";
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
  parseSettings,
  settingsPayload,
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

  useEffect(() => {
    let alive = true;
    void getSettings()
      .then((raw) => {
        if (!alive) return;
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

  /** Persist a patch via `set_settings`; revert on failure, never lie. */
  async function saveSettings(patch: Partial<SettingsState>) {
    if (settings === null) return;
    const next: SettingsState = { ...settings, ...patch };
    const previous = settings;
    setSettingsState(next);
    try {
      await setSettings({ settings: settingsPayload(next) });
      setStatus(st(t, "settings.saved", "Saved."));
    } catch {
      setSettingsState(previous);
      setStatus(st(t, "settings.saveFailed", "Could not save settings."));
    }
  }

  async function handleModuleToggle(id: ModuleId, enabled: boolean) {
    if (settings === null) return;
    await saveSettings({
      modules: { ...settings.modules, [id]: enabled },
    });
  }

  async function handleThemeChange(theme: ThemeId) {
    // Apply immediately for this session, then persist via the backend.
    setTheme(theme);
    await saveSettings({ theme });
  }

  async function handleLanguageChange(language: SupportedLanguage) {
    // Runtime switch, no restart — then persist via the backend.
    await changeLanguage(language);
    await saveSettings({ language });
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
