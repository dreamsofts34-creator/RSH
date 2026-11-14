import type { ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { TauriCommandError } from "@/lib/tauri";
import { cn } from "@/lib/utils";

/**
 * Shared bits for the Setup wizard steps (Phase E3 / UI plan P5).
 *
 * Every step shows real backend state and fails honestly: a backend call
 * that is not implemented yet renders the "not available yet" banner
 * instead of fake data, and any other failure renders its message.
 */

/** Split a `TauriCommandError` into "not implemented" vs. a message. */
export function describeError(error: unknown): {
  notImplemented: boolean;
  message: string;
} {
  if (error instanceof TauriCommandError) {
    return { notImplemented: error.notImplemented, message: error.message };
  }
  return {
    notImplemented: false,
    message: error instanceof Error ? error.message : String(error ?? ""),
  };
}

export type NoticeTone = "error" | "warn" | "info" | "ok";

/** Small status banner used by every step. */
export function Notice({
  tone,
  children,
}: {
  tone: NoticeTone;
  children: ReactNode;
}) {
  return (
    <div
      role={tone === "error" ? "alert" : "status"}
      className={cn(
        "rounded-[10px] border px-3 py-2 text-sm",
        tone === "error" &&
          "border-[var(--danger)]/40 bg-[var(--danger)]/10 text-[var(--ink)]",
        tone === "warn" &&
          "border-amber-500/40 bg-amber-500/10 text-[var(--ink)]",
        tone === "info" &&
          "border-[var(--line)] bg-[var(--accent-soft)] text-[var(--ink)]",
        tone === "ok" &&
          "border-emerald-500/40 bg-emerald-500/10 text-[var(--ink)]",
      )}
    >
      {children}
    </div>
  );
}

/** "The backend has not implemented this yet" — shown, never faked over. */
export function NotImplementedNotice() {
  const { t } = useTranslation();
  return <Notice tone="warn">{t("setup.notImplemented")}</Notice>;
}

/** Step heading + description, shared by every wizard step. */
export function StepIntro({
  title,
  description,
}: {
  title: string;
  description: string;
}) {
  return (
    <div className="space-y-1">
      <h2 className="text-lg font-bold text-[var(--ink)]">{title}</h2>
      <p className="text-sm text-[var(--muted)]">{description}</p>
    </div>
  );
}
