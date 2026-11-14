import { useEffect, useState } from "react";
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
  getTransportState,
  switchTransport,
  TauriCommandError,
  type ConnectionState,
  type TransportKind,
} from "@/lib/tauri";
import { st } from "./settings";

/**
 * Transport (UI plan P9.3): Wi-Fi / USB-serial choice + wheel address.
 * Read via `get_transport_state`, saved explicitly via
 * `switch_transport` (no IPC per keystroke — R-P4).
 */
export function TransportSection() {
  const { t } = useTranslation();
  const [loading, setLoading] = useState(true);
  const [unavailable, setUnavailable] = useState(false);
  const [loadFailed, setLoadFailed] = useState(false);
  const [transport, setTransport] = useState<TransportKind>("wifi");
  const [address, setAddress] = useState("");
  const [connection, setConnection] = useState<ConnectionState | null>(null);
  const [backendError, setBackendError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [status, setStatus] = useState<string | null>(null);

  useEffect(() => {
    let alive = true;
    void getTransportState()
      .then((state) => {
        if (!alive) return;
        setTransport(state.transport);
        setAddress(state.address);
        setConnection(state.state);
        setBackendError(state.error);
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

  async function handleSave() {
    setSaving(true);
    setStatus(null);
    try {
      const state = await switchTransport({ transport, address });
      setConnection(state.state);
      setBackendError(state.error);
      // H11: a switch reports "connecting" first — poll until it
      // settles (connected / error / disconnected) instead of showing
      // a stale "connecting" forever. 10 polls × 500 ms, then stop.
      if (state.state === "connecting") {
        setStatus(
          st(t, "settings.transport.connecting", "Connecting…"),
        );
        for (let attempt = 0; attempt < 10; attempt += 1) {
          await new Promise((resolve) => setTimeout(resolve, 500));
          try {
            const polled = await getTransportState();
            setConnection(polled.state);
            setBackendError(polled.error);
            if (polled.state !== "connecting") break;
          } catch {
            break;
          }
        }
      }
      setStatus(st(t, "settings.transport.saved", "Transport saved."));
    } catch (error) {
      if (error instanceof TauriCommandError && error.notImplemented) {
        setUnavailable(true);
      } else {
        setStatus(
          st(
            t,
            "settings.transport.saveFailed",
            "Could not save transport.",
          ),
        );
      }
    } finally {
      setSaving(false);
    }
  }

  const stateLabel =
    connection === null
      ? null
      : st(
          t,
          `settings.transport.state${connection[0].toUpperCase()}${connection.slice(1)}`,
          connection,
        );

  return (
    <Card>
      <CardHeader>
        <CardTitle>
          {st(t, "settings.transport.title", "Transport")}
        </CardTitle>
        <CardDescription>
          {st(
            t,
            "settings.transport.description",
            "How the app connects to the wheel.",
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
              "settings.transport.unavailable",
              "Transport settings are not available yet — the backend command is not implemented.",
            )}
          </p>
        ) : loadFailed ? (
          <p role="alert" className="text-sm text-[var(--error-text)]">
            {st(
              t,
              "settings.transport.loadFailed",
              "Could not load transport state.",
            )}
          </p>
        ) : (
          <>
            {connection !== null ? (
              <p className="text-sm">
                {st(t, "settings.transport.state", "Connection")}:{" "}
                <span className="font-semibold">{stateLabel}</span>
                {backendError !== null ? (
                  <span className="block text-[13px] text-[var(--error-text)]">
                    {backendError}
                  </span>
                ) : null}
              </p>
            ) : null}
            <div className="space-y-2">
              <Label htmlFor="settings-transport">
                {st(t, "settings.transport.kind", "Transport")}
              </Label>
              <select
                id="settings-transport"
                aria-label={st(t, "settings.transport.kind", "Transport")}
                value={transport}
                disabled={saving}
                onChange={(event) =>
                  setTransport(event.target.value as TransportKind)
                }
                className="h-10 w-full max-w-xs rounded-[10px] border border-[var(--line)] bg-[var(--card)] px-3 py-2 text-sm text-[var(--ink)] focus-visible:outline-2 focus-visible:outline-dashed focus-visible:outline-[var(--accent)] disabled:cursor-not-allowed disabled:opacity-50"
              >
                <option value="wifi">
                  {st(t, "settings.transport.wifi", "Wi-Fi")}
                </option>
                <option value="usb">
                  {st(t, "settings.transport.usb", "USB serial")}
                </option>
              </select>
            </div>
            <div className="space-y-2">
              <Label htmlFor="settings-address">
                {st(t, "settings.transport.address", "Wheel address")}
              </Label>
              <Input
                id="settings-address"
                value={address}
                disabled={saving}
                onChange={(event) => setAddress(event.target.value)}
                placeholder={
                  transport === "usb"
                    ? st(
                        t,
                        "settings.transport.addressPlaceholderUsb",
                        "Auto-detected",
                      )
                    : "192.168.4.1"
                }
              />
              <p className="text-[13px] text-[var(--muted)]">
                {transport === "usb"
                  ? st(
                      t,
                      "settings.transport.addressHintUsb",
                      "Leave empty — the serial port is auto-detected.",
                    )
                  : st(
                      t,
                      "settings.transport.addressHint",
                      "IP address for Wi-Fi, serial port for USB.",
                    )}
              </p>
            </div>
            <Button
              type="button"
              disabled={
                saving ||
                (transport === "wifi" && address.trim() === "")
              }
              onClick={() => void handleSave()}
            >
              {st(t, "settings.transport.save", "Connect")}
            </Button>
          </>
        )}
        {status !== null ? (
          <p aria-live="polite" className="text-sm text-[var(--muted)]">
            {status}
          </p>
        ) : null}
      </CardContent>
    </Card>
  );
}
