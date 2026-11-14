import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  getFirmwareVersion,
  uploadFirmware,
  validateFirmwareImage,
  type FirmwareValidation,
} from "@/lib/tauri";
import { Notice, NotImplementedNotice, StepIntro, describeError, type StepProps } from "./components";

/**
 * Step 6 — Firmware upload gate (closes the C# classic-mode gap
 * SEC-R2-01: the classic window had no firmware upload at all).
 *
 * Flow: file path -> `validate_firmware_image` (client-side Ed25519
 * signature validation, via the `rsh-security` verifier the coordinator
 * wires into the Tauri command) -> only on a valid signature AND no
 * downgrade does the Upload button enable -> `upload_firmware`.
 * An invalid signature, a downgrade, or a corrupt image shows a clear
 * error and the upload stays disabled. Nothing is ever uploaded
 * unvalidated.
 */

/**
 * Parse "a.b.c" into a triple, or null when it is not a version.
 *
 * The ≤255 cap is the contract, not an arbitrary limit: the backend
 * verifier parses versions as a `(u8, u8, u8)` triple
 * (`crates/rsh-security/src/firmware_verify.rs::parse_version`,
 * matching `ota_validate.h`), so a component above 255 can never be
 * a real firmware version — accepting it here would only let the
 * downgrade gate pass an image the backend then rejects.
 */
export function parseVersion(value: string): [number, number, number] | null {
  const parts = value.trim().split(".");
  if (parts.length !== 3) return null;
  const numbers = parts.map((part) => Number(part));
  if (
    parts.some((part) => !/^\d+$/.test(part)) ||
    numbers.some((n) => !Number.isInteger(n) || n < 0 || n > 255)
  ) {
    return null;
  }
  return [numbers[0], numbers[1], numbers[2]];
}

/** Compare two version triples: -1 / 0 / 1. */
export function compareVersions(
  a: [number, number, number],
  b: [number, number, number],
): number {
  for (let i = 0; i < 3; i += 1) {
    if (a[i] !== b[i]) return a[i] < b[i] ? -1 : 1;
  }
  return 0;
}

export function FirmwareStep({ onSuccessChange }: StepProps) {
  const { t } = useTranslation();
  const [currentVersion, setCurrentVersion] = useState<string | null>(null);
  const [versionUnknown, setVersionUnknown] = useState(false);
  const [versionFailed, setVersionFailed] = useState(false);
  const [versionNonce, setVersionNonce] = useState(0);
  const [path, setPath] = useState("");
  const [validation, setValidation] = useState<FirmwareValidation | null>(null);
  const [busy, setBusy] = useState<"validating" | "uploading" | null>(null);
  const [error, setError] = useState<{ notImplemented: boolean; message: string } | null>(
    null,
  );
  const [uploaded, setUploaded] = useState(false);
  const [pickerUnavailable, setPickerUnavailable] = useState(false);

  // H5: the step succeeds once a firmware image has uploaded.
  useEffect(() => {
    onSuccessChange?.(uploaded);
  }, [uploaded, onSuccessChange]);

  // The installed version is the downgrade baseline; without it the
  // upload gate cannot prove "not a downgrade", so it stays closed.
  // M13: a failed version read can be retried (versionNonce).
  useEffect(() => {
    let cancelled = false;
    setVersionFailed(false);
    getFirmwareVersion()
      .then((version) => {
        if (!cancelled) {
          setCurrentVersion(version);
          setVersionUnknown(false);
        }
      })
      .catch((caught: unknown) => {
        if (cancelled) return;
        if (describeError(caught).notImplemented) {
          setVersionUnknown(true);
        } else {
          setVersionFailed(true);
          setError(describeError(caught));
        }
      });
    return () => {
      cancelled = true;
    };
  }, [versionNonce]);

  function handleValidate() {
    setBusy("validating");
    setError(null);
    setUploaded(false);
    validateFirmwareImage({ path: path.trim() })
      .then(setValidation)
      .catch((caught: unknown) => {
        setValidation(null);
        setError(describeError(caught));
      })
      .finally(() => setBusy(null));
  }

  /**
   * M13: native file picker (`@tauri-apps/plugin-dialog`). The plugin
   * must also be registered on the Rust side
   * (`tauri_plugin_dialog::init()` + the `dialog:default` capability)
   * before this succeeds — until then the import resolves but `open()`
   * rejects, and the step falls back to the typed path with an honest
   * note instead of a dead button.
   */
  async function handleBrowse() {
    setPickerUnavailable(false);
    try {
      const { open } = await import("@tauri-apps/plugin-dialog");
      const selected = await open({
        multiple: false,
        filters: [
          {
            name: "Firmware image",
            extensions: ["bin"],
          },
        ],
      });
      if (typeof selected === "string" && selected.length > 0) {
        setPath(selected);
        setValidation(null);
        setUploaded(false);
      }
    } catch {
      setPickerUnavailable(true);
    }
  }

  function handleUpload() {
    setBusy("uploading");
    setError(null);
    uploadFirmware({ path: path.trim() })
      .then(() => setUploaded(true))
      .catch((caught: unknown) => setError(describeError(caught)))
      .finally(() => setBusy(null));
  }

  const imageVersion =
    validation?.version === null || validation?.version === undefined
      ? null
      : parseVersion(validation.version);
  const installedVersion =
    currentVersion === null ? null : parseVersion(currentVersion);
  const isDowngrade =
    imageVersion !== null &&
    installedVersion !== null &&
    compareVersions(imageVersion, installedVersion) < 0;
  const canUpload =
    validation !== null &&
    validation.valid &&
    validation.signatureValid &&
    imageVersion !== null &&
    installedVersion !== null &&
    !isDowngrade &&
    busy === null;

  return (
    <div className="space-y-4">
      <StepIntro
        title={t("setup.firmware.title")}
        description={t("setup.firmware.description")}
      />

      <p className="text-sm text-[var(--ink)]">
        {t("setup.firmware.currentVersion")}:{" "}
        {currentVersion ?? t("setup.firmware.unknownVersion")}
      </p>

      <div className="space-y-2">
        <Label htmlFor="setup-firmware-path">{t("setup.firmware.filePath")}</Label>
        <div className="flex flex-wrap gap-2">
          <Input
            id="setup-firmware-path"
            value={path}
            onChange={(event) => {
              setPath(event.target.value);
              setValidation(null);
              setUploaded(false);
            }}
            placeholder={t("setup.firmware.filePlaceholder")}
            className="max-w-md flex-1"
          />
          <Button type="button" variant="outline" onClick={() => void handleBrowse()}>
            {t("setup.firmware.browse")}
          </Button>
        </div>
        {pickerUnavailable ? (
          <Notice tone="warn">{t("setup.firmware.pickerUnavailable")}</Notice>
        ) : null}
      </div>

      {(versionUnknown || versionFailed) && (
        <Button
          type="button"
          variant="outline"
          onClick={() => setVersionNonce((n) => n + 1)}
        >
          {t("setup.firmware.retry")}
        </Button>
      )}

      <div className="flex flex-wrap gap-2">
        <Button
          type="button"
          onClick={handleValidate}
          disabled={busy !== null || path.trim().length === 0}
        >
          {busy === "validating"
            ? t("setup.firmware.validating")
            : t("setup.firmware.validate")}
        </Button>
        <Button
          type="button"
          onClick={handleUpload}
          disabled={!canUpload}
          title={canUpload ? undefined : t("setup.firmware.uploadDisabled")}
        >
          {busy === "uploading"
            ? t("setup.firmware.uploading")
            : t("setup.firmware.upload")}
        </Button>
      </div>

      {!canUpload && validation !== null && (
        <Notice tone="info">{t("setup.firmware.uploadDisabled")}</Notice>
      )}

      {validation !== null && (
        <div className="space-y-2" aria-live="polite">
          {validation.valid && validation.signatureValid && !isDowngrade ? (
            <Notice tone="ok">{t("setup.firmware.valid")}</Notice>
          ) : (
            <Notice tone="error">{t("setup.firmware.invalid")}</Notice>
          )}
          <dl className="space-y-1 text-sm text-[var(--ink)]">
            <div className="flex gap-2">
              <dt className="font-semibold">{t("setup.firmware.version")}:</dt>
              <dd>{validation.version ?? "—"}</dd>
            </div>
            <div className="flex gap-2">
              <dt className="font-semibold">
                {validation.signatureValid
                  ? t("setup.firmware.signatureValid")
                  : t("setup.firmware.signatureInvalid")}
              </dt>
            </div>
          </dl>
          {isDowngrade && (
            <Notice tone="error">
              {t("setup.firmware.downgrade", {
                version: validation.version ?? "",
                current: currentVersion ?? "",
              })}
            </Notice>
          )}
          {validation.errors.length > 0 && (
            <div className="space-y-1">
              <p className="text-sm font-semibold text-[var(--ink)]">
                {t("setup.firmware.errors")}
              </p>
              <ul className="list-disc space-y-1 pl-5 text-sm text-[var(--ink)]">
                {validation.errors.map((message) => (
                  <li key={message}>{message}</li>
                ))}
              </ul>
            </div>
          )}
        </div>
      )}

      {versionUnknown && (
        <Notice tone="warn">{t("setup.firmware.unknownVersion")}</Notice>
      )}

      {uploaded && <Notice tone="ok">{t("setup.firmware.uploaded")}</Notice>}
      {error?.notImplemented ? (
        <NotImplementedNotice />
      ) : error ? (
        <Notice tone="error">{error.message}</Notice>
      ) : null}
    </div>
  );
}
