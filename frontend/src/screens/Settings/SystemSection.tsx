import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Switch } from "@/components/ui/switch";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import {
  checkForUpdate,
  exportDiagnostics,
  getAutostart,
  getFirewallState,
  setAutostart,
  TauriCommandError,
} from "@/lib/tauri";
import { st } from "./settings";

/**
 * System (UI plan P9.3 / P2.3): autostart toggle + firewall rule state.
 *
 * Autostart is read/written via `get_autostart` / `set_autostart`.
 * Stale-entry repair has no observable state in the command surface
 * (the getter returns a bare boolean), so its status is reported as
 * unknown rather than invented — except when startup itself had to
 * report a repair failure, which the shell shows as a banner.
 *
 * Firewall state comes from `get_firewall_state` (H3: the section
 * used to claim no such command existed — it has existed since
 * Phase F). The backend's `detail` string is shown verbatim.
 */
export function SystemSection() {
  const { t } = useTranslation();
  const [loading, setLoading] = useState(true);
  const [unavailable, setUnavailable] = useState(false);
  const [loadFailed, setLoadFailed] = useState(false);
  const [autostart, setAutostartState] = useState(false);
  const [saving, setSaving] = useState(false);
  const [status, setStatus] = useState<string | null>(null);
  const [firewall, setFirewall] = useState<{
    rulePresent: boolean;
    detail: string;
  } | null>(null);
  const [firewallFailed, setFirewallFailed] = useState(false);
  // H9: update check + diagnostics export live here (Settings -> System).
  const [checkingUpdate, setCheckingUpdate] = useState(false);
  const [updateStatus, setUpdateStatus] = useState<string | null>(null);
  const [exportingDiagnostics, setExportingDiagnostics] = useState(false);
  const [diagnosticsStatus, setDiagnosticsStatus] = useState<string | null>(null);

  useEffect(() => {
    let alive = true;
    void getFirewallState()
      .then((state) => {
        if (!alive) return;
        setFirewall(state);
      })
      .catch(() => {
        if (!alive) return;
        setFirewallFailed(true);
      });
    return () => {
      alive = false;
    };
  }, []);

  useEffect(() => {
    let alive = true;
    void getAutostart()
      .then((enabled) => {
        if (!alive) return;
        setAutostartState(enabled);
        setLoading(false);
      })
      .catch((error: unknown) => {
        if (!alive) return;
        setLoading(false);
        if (error instanceof TauriCommandError && error.notImplemented) {
          setUnavailable(true);
        } else {
          setLoadFailed(true);
        }
      });
    return () => {
      alive = false;
    };
  }, []);

  async function handleCheckForUpdate() {
    setCheckingUpdate(true);
    setUpdateStatus(null);
    try {
      const update = await checkForUpdate();
      setUpdateStatus(
        update === null
          ? st(t, "settings.system.updateUpToDate", "You are up to date.")
          : st(t, "settings.system.updateAvailable", "Update {{version}} is available.", {
              version: update.version,
            }),
      );
    } catch {
      setUpdateStatus(
        st(
          t,
          "settings.system.updateCheckFailed",
          "Could not check for updates.",
        ),
      );
    } finally {
      setCheckingUpdate(false);
    }
  }

  async function handleExportDiagnostics() {
    setExportingDiagnostics(true);
    setDiagnosticsStatus(null);
    try {
      const path = await exportDiagnostics();
      setDiagnosticsStatus(
        st(
          t,
          "settings.system.diagnosticsExported",
          "Diagnostics saved to {{path}}.",
          { path },
        ),
      );
    } catch {
      setDiagnosticsStatus(
        st(
          t,
          "settings.system.diagnosticsFailed",
          "Could not export diagnostics.",
        ),
      );
    } finally {
      setExportingDiagnostics(false);
    }
  }

  async function handleToggle(enabled: boolean) {
    const previous = autostart;
    setAutostartState(enabled);
    setSaving(true);
    setStatus(null);
    try {
      const confirmed = await setAutostart({ enabled });
      setAutostartState(confirmed);
      setStatus(
        st(
          t,
          "settings.system.autostartSaved",
          "Autostart setting saved.",
        ),
      );
    } catch (error) {
      setAutostartState(previous);
      if (error instanceof TauriCommandError && error.notImplemented) {
        setUnavailable(true);
      } else {
        setStatus(
          st(
            t,
            "settings.system.autostartFailed",
            "Could not save the autostart setting.",
          ),
        );
      }
    } finally {
      setSaving(false);
    }
  }

  return (
    <Card>
      <CardHeader>
        <CardTitle>{st(t, "settings.system.title", "System")}</CardTitle>
        <CardDescription>
          {st(
            t,
            "settings.system.description",
            "Startup and operating system integration.",
          )}
        </CardDescription>
      </CardHeader>
      <CardContent className="space-y-4">
        {loading ? (
          <p className="text-sm text-[var(--muted)]">
            {st(t, "settings.loading", "Loading settings…")}
          </p>
        ) : unavailable ? (
          <p role="alert" className="text-sm text-[var(--error-text)]">
            {st(
              t,
              "settings.system.unavailable",
              "System settings are not available yet — the backend command is not implemented.",
            )}
          </p>
        ) : loadFailed ? (
          <p role="alert" className="text-sm text-[var(--error-text)]">
            {st(
              t,
              "settings.system.loadFailed",
              "Could not load system settings.",
            )}
          </p>
        ) : (
          <div className="flex items-center justify-between gap-3">
            <span className="text-sm font-medium">
              {st(
                t,
                "settings.system.autostart",
                "Start with Windows",
              )}
            </span>
            <Switch
              checked={autostart}
              disabled={saving}
              aria-label={st(
                t,
                "settings.system.autostart",
                "Start with Windows",
              )}
              onCheckedChange={(checked) => void handleToggle(checked)}
            />
          </div>
        )}

        <p className="text-[13px] text-[var(--muted)]">
          {st(
            t,
            "settings.system.repairUnknown",
            "Stale-entry repair status is not reported by the backend.",
          )}
        </p>

        <div className="flex items-center justify-between gap-3 rounded-[10px] border border-[var(--line)] px-3 py-2">
          <span className="text-sm font-medium">
            {st(t, "settings.system.firewall", "Firewall rule")}
          </span>
          {firewallFailed ? (
            <span className="text-[13px] text-[var(--error-text)]">
              {st(
                t,
                "settings.system.firewallLoadFailed",
                "Could not load the firewall rule state.",
              )}
            </span>
          ) : firewall === null ? (
            <span className="text-[13px] text-[var(--muted)]">
              {st(t, "settings.loading", "Loading settings…")}
            </span>
          ) : (
            <span className="text-right text-[13px]">
              <span className="font-medium">
                {firewall.rulePresent
                  ? st(
                      t,
                      "settings.system.firewallPresent",
                      "Firewall rule present",
                    )
                  : st(
                      t,
                      "settings.system.firewallAbsent",
                      "No firewall rule",
                    )}
              </span>
              <span className="block text-[var(--muted)]">
                {firewall.detail}
              </span>
            </span>
          )}
        </div>

        <div className="flex flex-wrap gap-2 border-t border-[var(--line)] pt-4">
          <Button
            type="button"
            variant="outline"
            disabled={checkingUpdate}
            onClick={() => void handleCheckForUpdate()}
          >
            {checkingUpdate
              ? st(
                  t,
                  "settings.system.checkingUpdate",
                  "Checking for updates…",
                )
              : st(t, "settings.system.checkUpdate", "Check for updates")}
          </Button>
          <Button
            type="button"
            variant="outline"
            disabled={exportingDiagnostics}
            onClick={() => void handleExportDiagnostics()}
          >
            {exportingDiagnostics
              ? st(
                  t,
                  "settings.system.exportingDiagnostics",
                  "Exporting diagnostics…",
                )
              : st(
                  t,
                  "settings.system.exportDiagnostics",
                  "Export diagnostics",
                )}
          </Button>
        </div>
        {updateStatus !== null ? (
          <p aria-live="polite" className="text-sm text-[var(--muted)]">
            {updateStatus}
          </p>
        ) : null}
        {diagnosticsStatus !== null ? (
          <p aria-live="polite" className="text-sm text-[var(--muted)]">
            {diagnosticsStatus}
          </p>
        ) : null}

        {status !== null ? (
          <p aria-live="polite" className="text-sm text-[var(--muted)]">
            {status}
          </p>
        ) : null}
      </CardContent>
    </Card>
  );
}
