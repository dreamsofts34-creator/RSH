import { useTranslation } from "react-i18next";
import { Contrast, Moon, Sun, Monitor } from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Label } from "@/components/ui/label";
import { SUPPORTED_LANGUAGES, type SupportedLanguage } from "@/lib/i18n";
import { type ThemeId } from "@/lib/theme";
import { cn } from "@/lib/utils";
import { st } from "./settings";

/**
 * Appearance (UI plan P9.3) — the ONLY place the theme and language
 * switches live. Unlike the shell's local switch, these persist through
 * the `set_settings` wrapper (via the callbacks), so the choice survives
 * restarts; the theme is also applied immediately for this session.
 */
export function AppearanceSection({
  theme,
  language,
  disabled,
  onThemeChange,
  onLanguageChange,
}: {
  theme: ThemeId;
  language: SupportedLanguage;
  disabled: boolean;
  onThemeChange: (theme: ThemeId) => void;
  onLanguageChange: (language: SupportedLanguage) => void;
}) {
  const { t } = useTranslation();

  const themeOptions: { id: ThemeId; label: string; icon: typeof Sun }[] = [
    { id: "light", label: st(t, "settings.appearance.light", "Light"), icon: Sun },
    { id: "dark", label: st(t, "settings.appearance.dark", "Dark"), icon: Moon },
    {
      id: "high-contrast",
      label: st(t, "settings.appearance.highContrast", "High contrast"),
      icon: Contrast,
    },
    {
      id: "system",
      label: st(t, "settings.appearance.system", "System"),
      icon: Monitor,
    },
  ];

  const languageOptions: { id: SupportedLanguage; label: string }[] =
    SUPPORTED_LANGUAGES.map((id) => ({
      id,
      label:
        id === "en"
          ? st(t, "settings.appearance.english", "English")
          : id === "ur"
            ? st(t, "settings.appearance.urdu", "Urdu")
            : st(t, "settings.appearance.hindi", "Hindi"),
    }));

  return (
    <Card>
      <CardHeader>
        <CardTitle>
          {st(t, "settings.appearance.title", "Appearance")}
        </CardTitle>
        <CardDescription>
          {st(
            t,
            "settings.appearance.description",
            "Theme and language. Changes apply immediately.",
          )}
        </CardDescription>
      </CardHeader>
      <CardContent className="space-y-5">
        <div className="space-y-2">
          <p className="text-xs font-semibold uppercase tracking-wide text-[var(--muted)]">
            {st(t, "settings.appearance.theme", "Theme")}
          </p>
          <div
            role="group"
            aria-label={st(t, "settings.appearance.theme", "Theme")}
            className="grid grid-cols-2 gap-2 sm:grid-cols-4"
          >
            {themeOptions.map(({ id, label, icon: Icon }) => (
              <Button
                key={id}
                type="button"
                variant={theme === id ? "secondary" : "outline"}
                size="sm"
                aria-pressed={theme === id}
                disabled={disabled}
                onClick={() => onThemeChange(id)}
                className={cn(
                  "justify-start",
                  theme === id && "border-[var(--accent)]",
                )}
              >
                <Icon className="h-4 w-4" aria-hidden="true" />
                {label}
              </Button>
            ))}
          </div>
        </div>
        <div className="space-y-2">
          <Label htmlFor="settings-language">
            {st(t, "settings.appearance.language", "Language")}
          </Label>
          <select
            id="settings-language"
            aria-label={st(t, "settings.appearance.language", "Language")}
            value={language}
            disabled={disabled}
            onChange={(event) =>
              onLanguageChange(event.target.value as SupportedLanguage)
            }
            className="h-10 w-full max-w-xs rounded-[10px] border border-[var(--line)] bg-[var(--card)] px-3 py-2 text-sm text-[var(--ink)] focus-visible:outline-2 focus-visible:outline-dashed focus-visible:outline-[var(--accent)] disabled:cursor-not-allowed disabled:opacity-50"
          >
            {languageOptions.map(({ id, label }) => (
              <option key={id} value={id}>
                {label}
              </option>
            ))}
          </select>
        </div>
      </CardContent>
    </Card>
  );
}
