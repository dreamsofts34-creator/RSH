import { useTranslation } from "react-i18next";
import { Switch } from "@/components/ui/switch";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import {
  MODULE_IDS,
  moduleLabelKey,
  st,
  type ModuleId,
  type ModuleStates,
} from "./settings";

/**
 * Per-module on/off toggles (UI plan P9.1) — the ONLY place modules are
 * toggled. Tuning cards only display this same state; the persisted
 * `modules` map in settings is the single source of truth.
 */
export function ModuleToggles({
  modules,
  disabled,
  onToggle,
}: {
  modules: ModuleStates;
  disabled: boolean;
  onToggle: (id: ModuleId, enabled: boolean) => void;
}) {
  const { t } = useTranslation();
  return (
    <Card>
      <CardHeader>
        <CardTitle>{st(t, "settings.modules.title", "Modules")}</CardTitle>
        <CardDescription>
          {st(
            t,
            "settings.modules.description",
            "Turn modules on or off. Changes apply immediately.",
          )}
        </CardDescription>
      </CardHeader>
      <CardContent>
        <ul className="divide-y divide-[var(--line)]">
          {MODULE_IDS.map((id) => {
            const label = st(t, moduleLabelKey(id), id);
            return (
              <li key={id} className="flex items-center justify-between py-3">
                <span className="text-sm font-medium">{label}</span>
                <Switch
                  checked={modules[id]}
                  disabled={disabled}
                  aria-label={label}
                  onCheckedChange={(checked) => onToggle(id, checked)}
                />
              </li>
            );
          })}
        </ul>
      </CardContent>
    </Card>
  );
}
