import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import {
  beginPairing,
  getPairingStatus,
  type PairingStatus,
} from "@/lib/tauri";
import { Notice, NotImplementedNotice, StepIntro, describeError } from "./components";

/**
 * Step 2 — Pairing (UI plan P5.2, Security plan contract).
 *
 * `begin_pairing` opens the 60 s window and returns the one-time device
 * password. The password is shown ONCE, grouped in 4 groups of 6
 * characters, with a clear "shown once" warning. Once the user confirms
 * they saved it, the password is dropped from component state and is
 * never fetched or shown again in this session.
 */

/** "abcdefghijklmnopqrstuvwx" -> "abcdef ghijkl mnopqr stuvwx". */
export function groupPassword(password: string): string {
  return password.replace(/(.{6})/g, "$1 ").trim();
}

export function PairingStep() {
  const { t } = useTranslation();
  const [status, setStatus] = useState<PairingStatus | null>(null);
  const [secondsLeft, setSecondsLeft] = useState(0);
  const [acknowledged, setAcknowledged] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<{ notImplemented: boolean; message: string } | null>(
    null,
  );

  // Local 1 s countdown from the window the backend reported.
  useEffect(() => {
    if (secondsLeft <= 0) return undefined;
    const timer = window.setInterval(() => {
      setSecondsLeft((left) => Math.max(0, left - 1));
    }, 1000);
    return () => window.clearInterval(timer);
  }, [secondsLeft]);

  function handleBegin() {
    setBusy(true);
    setError(null);
    beginPairing()
      .then((next) => {
        setStatus(next);
        setSecondsLeft(next.secondsLeft);
        setAcknowledged(false);
      })
      .catch((caught: unknown) => setError(describeError(caught)))
      .finally(() => setBusy(false));
  }

  function handleRefresh() {
    setBusy(true);
    setError(null);
    getPairingStatus()
      .then((next) => {
        setStatus(next);
        setSecondsLeft(next.secondsLeft);
      })
      .catch((caught: unknown) => setError(describeError(caught)))
      .finally(() => setBusy(false));
  }

  function handleSaved() {
    // Shown once: drop the password from state; it is never re-fetched.
    setAcknowledged(true);
    setStatus((current) =>
      current === null ? null : { ...current, password: null },
    );
  }

  const password = acknowledged ? null : status?.password ?? null;

  return (
    <div className="space-y-4">
      <StepIntro
        title={t("setup.pairing.title")}
        description={t("setup.pairing.description")}
      />

      <div className="flex flex-wrap gap-2">
        <Button type="button" onClick={handleBegin} disabled={busy}>
          {t("setup.pairing.begin")}
        </Button>
        {!acknowledged && status !== null && (
          <Button type="button" variant="outline" onClick={handleRefresh} disabled={busy}>
            {t("setup.connect.refresh")}
          </Button>
        )}
      </div>

      {password !== null && (
        <div className="space-y-3">
          <Notice tone="warn">{t("setup.pairing.shownOnce")}</Notice>
          <p
            className="rounded-[10px] border border-[var(--line)] bg-[var(--card)] px-4 py-3 text-center font-mono text-lg font-bold tracking-wider text-[var(--ink)]"
            aria-label={t("setup.pairing.title")}
          >
            {groupPassword(password)}
          </p>
          <p aria-live="polite" className="text-sm text-[var(--ink)]">
            {t("setup.pairing.secondsLeft", { seconds: secondsLeft })}
          </p>
          <Button type="button" variant="secondary" onClick={handleSaved}>
            {t("setup.pairing.savedButton")}
          </Button>
        </div>
      )}

      {acknowledged && <Notice tone="info">{t("setup.pairing.hidden")}</Notice>}

      {password === null && !acknowledged && status !== null && (
        <Notice tone="info">{t("setup.pairing.noWindow")}</Notice>
      )}

      {error?.notImplemented ? (
        <NotImplementedNotice />
      ) : error ? (
        <Notice tone="error">{error.message}</Notice>
      ) : null}
    </div>
  );
}
