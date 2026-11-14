import { useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { setPassword } from "@/lib/tauri";
import { Notice, NotImplementedNotice, StepIntro, describeError } from "./components";

/**
 * Step 3 — Change password.
 *
 * Sets the device password via `set_password` (8..63 printable ASCII).
 * The password is validated client-side first (length, printable
 * characters, confirmation match) and is never displayed back or stored
 * anywhere in the UI.
 */

function isPrintableAscii(value: string): boolean {
  return /^[\x20-\x7e]*$/.test(value);
}

export function PasswordStep() {
  const { t } = useTranslation();
  const [password, setPasswordValue] = useState("");
  const [confirm, setConfirm] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<{ notImplemented: boolean; message: string } | null>(
    null,
  );
  const [saved, setSaved] = useState(false);

  function handleChange() {
    setSaved(false);
    if (password.length === 0) {
      setError({ notImplemented: false, message: t("setup.password.required") });
      return;
    }
    if (password.length < 8) {
      setError({ notImplemented: false, message: t("setup.password.tooShort") });
      return;
    }
    if (!isPrintableAscii(password)) {
      setError({ notImplemented: false, message: t("setup.password.invalidChars") });
      return;
    }
    if (password !== confirm) {
      setError({ notImplemented: false, message: t("setup.password.mismatch") });
      return;
    }
    setBusy(true);
    setError(null);
    setPassword({ password })
      .then(() => {
        setSaved(true);
        setPasswordValue("");
        setConfirm("");
      })
      .catch((caught: unknown) => setError(describeError(caught)))
      .finally(() => setBusy(false));
  }

  return (
    <div className="space-y-4">
      <StepIntro
        title={t("setup.password.title")}
        description={t("setup.password.description")}
      />

      <div className="space-y-2">
        <Label htmlFor="setup-new-password">{t("setup.password.new")}</Label>
        <Input
          id="setup-new-password"
          type="password"
          autoComplete="new-password"
          maxLength={63}
          value={password}
          onChange={(event) => setPasswordValue(event.target.value)}
        />
      </div>

      <div className="space-y-2">
        <Label htmlFor="setup-confirm-password">{t("setup.password.confirm")}</Label>
        <Input
          id="setup-confirm-password"
          type="password"
          autoComplete="new-password"
          maxLength={63}
          value={confirm}
          onChange={(event) => setConfirm(event.target.value)}
        />
      </div>

      <Button type="button" onClick={handleChange} disabled={busy}>
        {t("setup.password.change")}
      </Button>

      {saved && <Notice tone="ok">{t("setup.password.success")}</Notice>}
      {error?.notImplemented ? (
        <NotImplementedNotice />
      ) : error ? (
        <Notice tone="error">{error.message}</Notice>
      ) : null}
    </div>
  );
}
