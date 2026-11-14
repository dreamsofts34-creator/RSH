import { memo } from "react";
import { useTranslation } from "react-i18next";
import { Wifi, Usb, PlugZap } from "lucide-react";
import type { ConnectionState, TransportState } from "@/lib/tauri";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Badge } from "@/components/ui/badge";

export type ConnectionLoadState = "loading" | "notImplemented" | "error";

interface ConnectionCardProps {
  transport: TransportState | null;
  loadState: ConnectionLoadState | null;
  /** Live connection from the 30 Hz snapshot, when one has arrived. */
  liveConnection: ConnectionState | null;
}

function stateVariant(state: ConnectionState): "default" | "secondary" | "destructive" | "outline" | "success" {
  switch (state) {
    case "connected":
      return "success";
    case "error":
      return "destructive";
    case "disconnected":
      return "secondary";
    case "connecting":
    case "pairing":
      return "default";
  }
}

/** Connection status card: transport kind, board address, session state. */
function ConnectionCardInner({
  transport,
  loadState,
  liveConnection,
}: ConnectionCardProps) {
  const { t } = useTranslation();
  const state: ConnectionState | null =
    liveConnection ?? transport?.state ?? null;

  return (
    <Card>
      <CardHeader className="flex flex-row items-center justify-between space-y-0">
        <CardTitle>{t("dashboard.connection.title")}</CardTitle>
        <PlugZap className="h-4 w-4 text-[var(--muted)]" aria-hidden="true" />
      </CardHeader>
      <CardContent className="space-y-2 text-sm">
        {loadState === "loading" && !transport ? (
          <p className="text-[var(--muted)]">{t("dashboard.loading")}</p>
        ) : !transport ? (
          <p className="text-[var(--muted)]">
            {t("dashboard.connection.notAvailable")}
          </p>
        ) : (
          <>
            <div className="flex items-center justify-between gap-2">
              <span className="text-[var(--muted)]">
                {t("dashboard.connection.transport")}
              </span>
              <span className="inline-flex items-center gap-1.5 font-medium">
                {transport.transport === "wifi" ? (
                  <Wifi className="h-4 w-4" aria-hidden="true" />
                ) : (
                  <Usb className="h-4 w-4" aria-hidden="true" />
                )}
                {transport.transport === "wifi"
                  ? t("dashboard.connection.wifi")
                  : t("dashboard.connection.usb")}
              </span>
            </div>
            <div className="flex items-center justify-between gap-2">
              <span className="text-[var(--muted)]">
                {t("dashboard.connection.address")}
              </span>
              <span className="font-mono text-[13px] font-medium">
                {transport.address || "—"}
              </span>
            </div>
            {transport.error ? (
              <p className="text-[13px] text-[var(--error-text)]">
                {transport.error}
              </p>
            ) : null}
          </>
        )}
        <div className="flex items-center justify-between gap-2 pt-1">
          <span className="text-[var(--muted)]">
            {t("dashboard.connection.state")}
          </span>
          {state ? (
            <Badge variant={stateVariant(state)}>
              {t(`dashboard.connection.state${capitalize(state)}`)}
            </Badge>
          ) : (
            <span className="text-[var(--muted)]">
              {t("dashboard.notAvailable")}
            </span>
          )}
        </div>
      </CardContent>
    </Card>
  );
}

function capitalize(state: ConnectionState): string {
  return state.charAt(0).toUpperCase() + state.slice(1);
}

/** Memoised: the Dashboard re-renders at 30 Hz from the snapshot feed
 *  (H4) — this component only re-renders when its own props change. */
export const ConnectionCard = memo(ConnectionCardInner);
