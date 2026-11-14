import { useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  connectTransport,
  disconnectTransport,
  getTransportState,
  switchTransport,
  type ConnectionState,
  type TransportKind,
  type TransportState,
} from "@/lib/tauri";
import { Notice, NotImplementedNotice, StepIntro, describeError } from "./components";

/**
 * Step 1 — Connect (UI plan P5.2).
 *
 * Transport switch (Wi-Fi / USB-serial) + board address + connect, all
 * through the typed `tauri.ts` wrappers. The board is tried at `rsh.local`
 * first; if that fails, the step offers the 192.168.4.1 fallback (the
 * board's hotspot address). No discovery data is invented: until a
 * wrapper returns, the status is "disconnected".
 */

const DEFAULT_ADDRESS: Record<TransportKind, string> = {
  wifi: "rsh.local",
  usb: "",
};

function stateLabel(state: ConnectionState, t: (key: string) => string): string {
  switch (state) {
    case "connected":
      return t("setup.connect.connected");
    case "connecting":
      return t("setup.connect.connecting");
    case "pairing":
      return t("setup.connect.pairing");
    case "error":
      return t("setup.connect.error");
    case "disconnected":
    default:
      return t("setup.connect.disconnected");
  }
}

export function ConnectStep() {
  const { t } = useTranslation();
  const [transport, setTransport] = useState<TransportKind>("wifi");
  const [address, setAddress] = useState(DEFAULT_ADDRESS.wifi);
  const [status, setStatus] = useState<TransportState | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<{ notImplemented: boolean; message: string } | null>(
    null,
  );

  function pickTransport(next: TransportKind) {
    setTransport(next);
    setAddress(DEFAULT_ADDRESS[next]);
    setError(null);
  }

  async function run(action: () => Promise<TransportState>) {
    // Wi-Fi needs an address; USB auto-detects the board port, so an
    // empty address is fine there (H3: requiring one made USB
    // connect impossible — the default USB address is empty).
    if (transport === "wifi" && address.trim().length === 0) {
      setError({ notImplemented: false, message: t("setup.connect.addressRequired") });
      return;
    }
    setBusy(true);
    setError(null);
    try {
      setStatus(await action());
    } catch (caught) {
      setError(describeError(caught));
    } finally {
      setBusy(false);
    }
  }

  function handleConnect() {
    void run(() => connectTransport({ transport, address: address.trim() }));
  }

  function handleSwitch() {
    void run(() => switchTransport({ transport, address: address.trim() }));
  }

  function handleRefresh() {
    setBusy(true);
    setError(null);
    getTransportState()
      .then(setStatus)
      .catch((caught: unknown) => setError(describeError(caught)))
      .finally(() => setBusy(false));
  }

  function handleDisconnect() {
    setBusy(true);
    setError(null);
    disconnectTransport()
      .then(setStatus)
      .catch((caught: unknown) => setError(describeError(caught)))
      .finally(() => setBusy(false));
  }

  function handleTryFallback() {
    setAddress("192.168.4.1");
    setBusy(true);
    setError(null);
    connectTransport({ transport, address: "192.168.4.1" })
      .then(setStatus)
      .catch((caught: unknown) => setError(describeError(caught)))
      .finally(() => setBusy(false));
  }

  const showFallback =
    transport === "wifi" &&
    address.trim() === "rsh.local" &&
    status?.state === "error";

  return (
    <div className="space-y-4">
      <StepIntro
        title={t("setup.connect.title")}
        description={t("setup.connect.description")}
      />

      <div className="space-y-2">
        <Label>{t("setup.connect.transport")}</Label>
        <div className="flex gap-2" role="group" aria-label={t("setup.connect.transport")}>
          <Button
            type="button"
            variant={transport === "wifi" ? "default" : "outline"}
            onClick={() => pickTransport("wifi")}
          >
            {t("setup.connect.wifi")}
          </Button>
          <Button
            type="button"
            variant={transport === "usb" ? "default" : "outline"}
            onClick={() => pickTransport("usb")}
          >
            {t("setup.connect.usb")}
          </Button>
        </div>
      </div>

      <div className="space-y-2">
        <Label htmlFor="setup-address">{t("setup.connect.address")}</Label>
        <Input
          id="setup-address"
          value={address}
          onChange={(event) => setAddress(event.target.value)}
          placeholder={
            transport === "wifi"
              ? t("setup.connect.addressPlaceholderWifi")
              : t("setup.connect.addressPlaceholderUsb")
          }
        />
      </div>

      <div className="flex flex-wrap gap-2">
        <Button type="button" onClick={handleConnect} disabled={busy}>
          {t("setup.connect.connect")}
        </Button>
        <Button type="button" variant="secondary" onClick={handleSwitch} disabled={busy}>
          {t("setup.connect.switch")}
        </Button>
        <Button type="button" variant="outline" onClick={handleRefresh} disabled={busy}>
          {t("setup.connect.refresh")}
        </Button>
        {status?.state === "connected" && (
          <Button type="button" variant="outline" onClick={handleDisconnect} disabled={busy}>
            {t("setup.connect.disconnected")}
          </Button>
        )}
      </div>

      {showFallback && (
        <Button type="button" variant="outline" onClick={handleTryFallback} disabled={busy}>
          {t("setup.connect.tryFallback")}
        </Button>
      )}

      <p aria-live="polite" className="text-sm text-[var(--ink)]">
        {t("setup.connect.status")}:{" "}
        {status ? stateLabel(status.state, t) : t("setup.connect.disconnected")}
        {status?.address ? ` — ${status.address}` : ""}
        {status?.error ? ` — ${status.error}` : ""}
      </p>

      {error?.notImplemented ? (
        <NotImplementedNotice />
      ) : error ? (
        <Notice tone="error">{error.message}</Notice>
      ) : null}
    </div>
  );
}
