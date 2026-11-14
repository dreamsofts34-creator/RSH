import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Users, AlertTriangle, RefreshCw, Trash2, Copy, Check } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Badge } from "@/components/ui/badge";
import {
  TauriCommandError,
  activateProfile,
  createProfile,
  deleteProfile,
  duplicateProfile,
  getActiveProfile,
  listProfiles,
  type ProfileSummary,
} from "@/lib/tauri";

export const path = "profiles";
export const label = "Profiles";
export const icon = Users;

type LoadState =
  | { status: "loading" }
  | { status: "ready"; profiles: ProfileSummary[] }
  | { status: "unavailable"; message: string }
  | { status: "error"; message: string };

/** Profile names: non-empty, max 64 chars (feature inventory §3, SEC-C-15). */
export function isValidProfileName(name: string): boolean {
  const trimmed = name.trim();
  return trimmed.length > 0 && trimmed.length <= 64;
}

export function component() {
  return <ProfilesScreen />;
}

export function ProfilesScreen() {
  const { t } = useTranslation();
  const [load, setLoad] = useState<LoadState>({ status: "loading" });
  const [newName, setNewName] = useState("");
  const [nameError, setNameError] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [busyId, setBusyId] = useState<string | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<ProfileSummary | null>(null);

  const loadProfiles = useCallback(async () => {
    setLoad({ status: "loading" });
    try {
      const profiles = await listProfiles();
      // Trust the backend's active flags; fall back to get_active_profile
      // only if no profile reports itself active.
      const withActive = profiles.some((p) => p.active)
        ? profiles
        : await getActiveProfile().then((active) =>
            active
              ? profiles.map((p) => ({ ...p, active: p.id === active.id }))
              : profiles,
          );
      setLoad({ status: "ready", profiles: withActive });
    } catch (error) {
      if (error instanceof TauriCommandError && error.notImplemented) {
        setLoad({
          status: "unavailable",
          message: t("profiles.notAvailable", {
            defaultValue: "Profiles are not available yet.",
          }),
        });
      } else {
        setLoad({
          status: "error",
          message:
            error instanceof Error
              ? error.message
              : t("profiles.error", { defaultValue: "Could not load profiles." }),
        });
      }
    }
  }, [t]);

  useEffect(() => {
    void loadProfiles();
  }, [loadProfiles]);

  const runAction = async (
    id: string,
    action: (args: { id: string }) => Promise<unknown>,
    failureMessage: string,
  ) => {
    setBusyId(id);
    setActionError(null);
    try {
      await action({ id });
      await loadProfiles();
    } catch (error) {
      setActionError(
        error instanceof Error ? error.message : failureMessage,
      );
    } finally {
      setBusyId(null);
    }
  };

  const onCreate = async () => {
    if (!isValidProfileName(newName)) {
      setNameError(
        t("profiles.nameRequired", { defaultValue: "Enter a profile name." }),
      );
      return;
    }
    setNameError(null);
    setActionError(null);
    setBusyId("create");
    try {
      await createProfile({ name: newName.trim() });
      setNewName("");
      await loadProfiles();
    } catch (error) {
      setActionError(
        error instanceof Error
          ? error.message
          : t("profiles.createError", {
              defaultValue: "Could not create the profile.",
            }),
      );
    } finally {
      setBusyId(null);
    }
  };

  const confirmDelete = async () => {
    if (!deleteTarget) return;
    const target = deleteTarget;
    setDeleteTarget(null);
    await runAction(
      target.id,
      (args) => deleteProfile(args),
      t("profiles.deleteError", {
        defaultValue: "Could not delete the profile.",
      }),
    );
  };

  const profiles = load.status === "ready" ? load.profiles : [];

  return (
    <section aria-label={t("profiles.title", { defaultValue: "Profiles" })} className="space-y-6">
      <header>
        <h1 className="text-[26px] font-bold leading-tight">
          {t("profiles.title", { defaultValue: "Profiles" })}
        </h1>
        <p className="mt-1 max-w-2xl text-sm text-[var(--muted)]">
          {t("profiles.description", {
            defaultValue:
              "Profiles hold per-module tuning, calibration and pin settings. The active profile is used by the wheel.",
          })}
        </p>
      </header>

      {load.status === "loading" ? (
        <p className="text-sm text-[var(--muted)]">
          {t("profiles.loading", { defaultValue: "Loading profiles…" })}
        </p>
      ) : null}

      {load.status === "unavailable" ? (
        <Card>
          <CardContent className="flex items-start gap-3 pt-5">
            <AlertTriangle className="mt-0.5 h-5 w-5 shrink-0 text-[var(--warning-text)]" aria-hidden="true" />
            <p className="text-sm">{load.message}</p>
          </CardContent>
        </Card>
      ) : null}

      {load.status === "error" ? (
        <Card>
          <CardContent className="flex flex-col items-start gap-3 pt-5">
            <p className="text-sm text-[var(--error-text)]">{load.message}</p>
            <Button variant="outline" size="sm" onClick={() => void loadProfiles()}>
              <RefreshCw className="h-4 w-4" aria-hidden="true" />
              {t("profiles.retry", { defaultValue: "Retry" })}
            </Button>
          </CardContent>
        </Card>
      ) : null}

      {load.status === "ready" ? (
        <>
          <Card>
            <CardHeader>
              <CardTitle>
                {t("profiles.create", { defaultValue: "Create profile" })}
              </CardTitle>
            </CardHeader>
            <CardContent>
              <div className="flex flex-col gap-2 sm:flex-row sm:items-start">
                <div className="flex-1 space-y-1">
                  <Label htmlFor="new-profile-name">
                    {t("profiles.nameLabel", { defaultValue: "Profile name" })}
                  </Label>
                  <Input
                    id="new-profile-name"
                    value={newName}
                    maxLength={64}
                    onChange={(event) => setNewName(event.target.value)}
                    onKeyDown={(event) => {
                      if (event.key === "Enter") void onCreate();
                    }}
                    placeholder={t("profiles.nameLabel", { defaultValue: "Profile name" })}
                  />
                  {nameError ? (
                    <p role="alert" className="text-sm text-[var(--error-text)]">
                      {nameError}
                    </p>
                  ) : null}
                </div>
                <Button
                  type="button"
                  onClick={() => void onCreate()}
                  disabled={busyId === "create"}
                  className="sm:mt-6"
                >
                  {t("profiles.create", { defaultValue: "Create profile" })}
                </Button>
              </div>
            </CardContent>
          </Card>

          {actionError ? (
            <p role="alert" className="text-sm text-[var(--error-text)]">
              {actionError}
            </p>
          ) : null}

          {profiles.length === 0 ? (
            <p className="text-sm text-[var(--muted)]">
              {t("profiles.noProfiles", {
                defaultValue: "No profiles yet. Create one to get started.",
              })}
            </p>
          ) : (
            <ul className="space-y-2">
              {profiles.map((profile) => (
                <li key={profile.id}>
                  <Card>
                    <CardContent className="flex flex-col gap-3 pt-5 sm:flex-row sm:items-center sm:justify-between">
                      <div className="min-w-0">
                        <div className="flex flex-wrap items-center gap-2">
                          <span className="truncate text-[15px] font-bold">
                            {profile.name}
                          </span>
                          {profile.active ? (
                            <Badge variant="success">
                              {t("profiles.active", { defaultValue: "Active" })}
                            </Badge>
                          ) : null}
                        </div>
                        <p className="mt-1 text-sm text-[var(--muted)]">
                          {profile.game ??
                            t("profiles.noGame", { defaultValue: "No game bound" })}
                        </p>
                      </div>
                      <div className="flex shrink-0 flex-wrap gap-2">
                        <Button
                          type="button"
                          variant={profile.active ? "secondary" : "default"}
                          size="sm"
                          disabled={profile.active || busyId === profile.id}
                          onClick={() =>
                            void runAction(
                              profile.id,
                              (args) => activateProfile(args),
                              t("profiles.error", {
                                defaultValue: "Could not load profiles.",
                              }),
                            )
                          }
                        >
                          <Check className="h-4 w-4" aria-hidden="true" />
                          {t("profiles.activate", { defaultValue: "Activate" })}
                        </Button>
                        <Button
                          type="button"
                          variant="outline"
                          size="sm"
                          disabled={busyId === profile.id}
                          onClick={() =>
                            void runAction(
                              profile.id,
                              (args) => duplicateProfile(args),
                              t("profiles.duplicateError", {
                                defaultValue: "Could not duplicate the profile.",
                              }),
                            )
                          }
                        >
                          <Copy className="h-4 w-4" aria-hidden="true" />
                          {t("profiles.duplicate", { defaultValue: "Duplicate" })}
                        </Button>
                        <Button
                          type="button"
                          variant="destructive"
                          size="sm"
                          disabled={busyId === profile.id}
                          onClick={() => setDeleteTarget(profile)}
                        >
                          <Trash2 className="h-4 w-4" aria-hidden="true" />
                          {t("profiles.delete", { defaultValue: "Delete" })}
                        </Button>
                      </div>
                    </CardContent>
                  </Card>
                </li>
              ))}
            </ul>
          )}

          <Card>
            <CardHeader>
              <CardTitle>
                {t("profiles.backupGenerations", { defaultValue: "Backup generations" })}
              </CardTitle>
            </CardHeader>
            <CardContent className="space-y-3">
              <p className="text-sm text-[var(--muted)]">
                {t("profiles.backupNote", {
                  defaultValue:
                    "Every save rotates current → .bak1 → .bak2. Only the current generation is live; older generations are kept as backups.",
                })}
              </p>
              <ul className="space-y-2">
                {[
                  { key: "current", live: true },
                  { key: ".bak1", live: false },
                  { key: ".bak2", live: false },
                ].map((gen) => (
                  <li key={gen.key} className="flex items-center gap-2 text-sm">
                    <Badge variant={gen.live ? "success" : "outline"}>
                      {gen.live
                        ? t("profiles.live", { defaultValue: "Live" })
                        : t("profiles.generationsUnknown", {
                            defaultValue: "Not reported yet",
                          })}
                    </Badge>
                    <span className="font-mono text-[13px]">{gen.key}</span>
                  </li>
                ))}
              </ul>
            </CardContent>
          </Card>
        </>
      ) : null}

      <Dialog
        open={deleteTarget !== null}
        onOpenChange={(open) => {
          if (!open) setDeleteTarget(null);
        }}
      >
        <DialogContent aria-label={t("profiles.deleteTitle", { defaultValue: "Delete profile" })}>
          <DialogHeader>
            <DialogTitle>
              {t("profiles.deleteTitle", { defaultValue: "Delete profile" })}
            </DialogTitle>
            <DialogDescription>
              {t("profiles.deleteConfirm", {
                name: deleteTarget?.name ?? "",
                defaultValue: `Delete profile ${deleteTarget?.name ?? ""}? This cannot be undone.`,
              })}
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button type="button" variant="outline" onClick={() => setDeleteTarget(null)}>
              {t("profiles.cancel", { defaultValue: "Cancel" })}
            </Button>
            <Button type="button" variant="destructive" onClick={() => void confirmDelete()}>
              {t("profiles.delete", { defaultValue: "Delete" })}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </section>
  );
}

export default ProfilesScreen;
