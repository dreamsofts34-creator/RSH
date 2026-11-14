import { useTranslation } from "react-i18next";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { ScrollArea } from "@/components/ui/scroll-area";

export interface LogEntry {
  id: number;
  time: string;
  text: string;
}

/** Event log (capped at 100 entries, newest first). */
export function EventLog({ entries }: { entries: LogEntry[] }) {
  const { t } = useTranslation();
  return (
    <Card>
      <CardHeader>
        <CardTitle>{t("monitor.log.title")}</CardTitle>
      </CardHeader>
      <CardContent>
        {entries.length === 0 ? (
          <p className="text-sm text-[var(--muted)]">{t("monitor.log.empty")}</p>
        ) : (
          <ScrollArea className="h-48">
            <ul className="space-y-1.5 pr-3 text-sm">
              {entries.map((entry) => (
                <li key={entry.id} className="flex gap-3">
                  <span className="shrink-0 font-mono text-[13px] text-[var(--muted)]">
                    {entry.time}
                  </span>
                  <span>{entry.text}</span>
                </li>
              ))}
            </ul>
          </ScrollArea>
        )}
      </CardContent>
    </Card>
  );
}
