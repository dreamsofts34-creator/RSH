import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  backupNow,
  listBackups,
  openConfigFolder,
  restoreBackup,
  type BackupInfo,
} from "@/lib/tauri";
import { st } from "./settings";

/**
 * Data (UI plan P9.3): Backup now / Restore / Open config folder.
 *
 * The 3-generation backup list comes from `list_backups` (H3: the
 * section used to claim no such command existed — it has existed
 * since Phase F). Restore takes a generation file name
 * (`profiles.json`, `profiles.json.bak1`, `profiles.json.bak2` — the
 * backend resolves by file name) or a path typed in.
 */
const GENERATION_FILE: Record<string, string> = {
  current: "profiles.json",
  bak1: "profiles.json.bak1",
  bak2: "profiles.json.bak2",
};

function generationLabelKey(generation: string): string {
  switch (generation) {
    case "bak1":
      return "settings.data.generationBak1";
    case "bak2":
      return "settings.data.generationBak2";
    case "current":
    default:
      return "settings.data.generationCurrent";
  }
}

export function DataSection() {
  const { t } = useTranslation();
  const [backups, setBackups] = useState<BackupInfo[] | null>(null);
  const [loadFailed, setLoadFailed] = useState(false);
  const [restorePath, setRestorePath] = useState("");
  const [busy, setBusy] = useState(false);
  const [status, setStatus] = useState<string | null>(null);

  const refreshBackups = useCallback(async () => {
    try {
      setBackups(await listBackups());
    } catch {
      setLoadFailed(true);
    }
  }, []);

  useEffect(() => {
    void refreshBackups();
  }, [refreshBackups]);

  async function handleBackup() {
    setBusy(true);
    setStatus(null);
    try {
      await backupNow();
      await refreshBackups();
      setStatus(st(t, "settings.data.backupSaved", "Backup saved."));
    } catch {
      setStatus(
        st(t, "settings.data.backupFailed", "Could not create a backup."),
      );
    } finally {
      setBusy(false);
    }
  }

  /** Format an RFC 3339 `savedAt` for display (M6): a raw ISO
   *  string is not a readable date. Falls back to the raw value when
   *  it does not parse. */
  function formatSavedAt(savedAt: string): string {
    const date = new Date(savedAt);
    if (Number.isNaN(date.getTime())) return savedAt;
    return date.toLocaleString();
  }

  async function handleRestore(path: string) {
    const trimmed = path.trim();
    if (trimmed === "") return;
    // M6: restoring the CURRENT generation is a no-op — it is already
    // live. Say so instead of calling the backend and reporting a
    // success that changed nothing.
    if (trimmed === GENERATION_FILE["current"]) {
      setStatus(
        st(
          t,
          "settings.data.restoreCurrentNoop",
          "The current generation is already live — nothing to restore.",
        ),
      );
      return;
    }
    setBusy(true);
    setStatus(null);
    try {
      await restoreBackup({ path: trimmed });
      await refreshBackups();
      setStatus(st(t, "settings.data.restored", "Backup restored."));
    } catch (error) {
      // M6: show the backend's reason, not just a generic failure.
      const detail =
        error instanceof Error ? error.message : null;
      setStatus(
        detail
          ? st(
              t,
              "settings.data.restoreFailedDetail",
              "Could not restore the backup: {{error}}",
              { error: detail },
            )
          : st(
              t,
              "settings.data.restoreFailed",
              "Could not restore the backup.",
            ),
      );
    } finally {
      setBusy(false);
    }
  }

  async function handleOpenFolder() {
    try {
      await openConfigFolder();
    } catch {
      setStatus(
        st(
          t,
          "settings.data.openFolderFailed",
          "Could not open the config folder.",
        ),
      );
    }
  }

  const presentBackups = (backups ?? []).filter((b) => b.present);

  return (
    <Card>
      <CardHeader>
        <CardTitle>{st(t, "settings.data.title", "Data")}</CardTitle>
        <CardDescription>
          {st(
            t,
            "settings.data.description",
            "Back up and restore configuration data.",
          )}
        </CardDescription>
      </CardHeader>
      <CardContent className="space-y-4">
        <div className="flex flex-wrap gap-2">
          <Button
            type="button"
            disabled={busy}
            onClick={() => void handleBackup()}
          >
            {st(t, "settings.data.backupNow", "Back up now")}
          </Button>
          <Button
            type="button"
            variant="outline"
            disabled={busy}
            onClick={() => void handleOpenFolder()}
          >
            {st(t, "settings.data.openFolder", "Open config folder")}
          </Button>
        </div>

        {loadFailed ? (
          <p role="alert" className="text-sm text-[var(--error-text)]">
            {st(
              t,
              "settings.data.loadFailed",
              "Could not load the backup list.",
            )}
          </p>
        ) : backups === null ? (
          <p className="text-sm text-[var(--muted)]">
            {st(t, "settings.loading", "Loading settings…")}
          </p>
        ) : presentBackups.length === 0 ? (
          <p className="text-sm text-[var(--muted)]">
            {st(t, "settings.data.noBackups", "No backups saved yet.")}
          </p>
        ) : (
          <div className="space-y-2">
            <p className="text-sm font-semibold">
              {st(t, "settings.data.generations", "Saved backups")}
            </p>
            <ul className="space-y-2">
              {presentBackups.map((backup) => (
                <li
                  key={backup.generation}
                  className="flex items-center justify-between gap-3 rounded-[10px] border border-[var(--line)] px-3 py-2"
                >
                  <span className="min-w-0 text-sm">
                    {st(
                      t,
                      generationLabelKey(backup.generation),
                      backup.generation,
                    )}
                    {backup.savedAt !== null ? (
                      <span className="block truncate text-[13px] text-[var(--muted)]">
                        {st(t, "settings.data.savedAt", "Saved {{date}}.", {
                          date: formatSavedAt(backup.savedAt),
                        })}
                      </span>
                    ) : null}
                  </span>
                  <Button
                    type="button"
                    size="sm"
                    variant="outline"
                    disabled={busy}
                    onClick={() =>
                      void handleRestore(
                        GENERATION_FILE[backup.generation] ??
                          `profiles.json.${backup.generation}`,
                      )
                    }
                  >
                    {st(t, "settings.data.restore", "Restore")}
                  </Button>
                </li>
              ))}
            </ul>
          </div>
        )}

        <div className="space-y-2">
          <Label htmlFor="settings-restore-path">
            {st(t, "settings.data.restorePath", "Backup file")}
          </Label>
          <div className="flex flex-wrap gap-2">
            <Input
              id="settings-restore-path"
              value={restorePath}
              disabled={busy}
              onChange={(event) => setRestorePath(event.target.value)}
              placeholder={st(
                t,
                "settings.data.restorePathPlaceholder",
                "Path to a backup file",
              )}
              className="max-w-md flex-1"
            />
            <Button
              type="button"
              variant="outline"
              disabled={busy || restorePath.trim() === ""}
              onClick={() => void handleRestore(restorePath)}
            >
              {st(t, "settings.data.restore", "Restore")}
            </Button>
          </div>
        </div>

        {status !== null ? (
          <p aria-live="polite" className="text-sm text-[var(--muted)]">
            {status}
          </p>
        ) : null}
      </CardContent>
    </Card>
  );
}
