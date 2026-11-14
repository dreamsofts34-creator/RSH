import { useTranslation } from "react-i18next";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Badge } from "@/components/ui/badge";

/** Live button state grid, fed by the `rsh://button-press` event. */
export function ButtonGrid({
  buttons,
}: {
  /** Button number -> currently pressed. Only buttons the backend has reported appear. */
  buttons: Record<number, boolean>;
}) {
  const { t } = useTranslation();
  const numbers = Object.keys(buttons)
    .map(Number)
    .sort((a, b) => a - b);

  return (
    <Card>
      <CardHeader>
        <CardTitle>{t("monitor.buttons.title")}</CardTitle>
      </CardHeader>
      <CardContent>
        {numbers.length === 0 ? (
          <p className="text-sm text-[var(--muted)]">
            {t("monitor.buttons.empty")}
          </p>
        ) : (
          <ul className="grid grid-cols-4 gap-2 sm:grid-cols-6 lg:grid-cols-8">
            {numbers.map((n) => {
              const pressed = buttons[n] ?? false;
              return (
                <li
                  key={n}
                  className="flex flex-col items-center gap-1 rounded-lg border border-[var(--line)] px-2 py-2"
                >
                  <span className="font-mono text-sm font-semibold">{n}</span>
                  <Badge variant={pressed ? "default" : "secondary"}>
                    {pressed
                      ? t("monitor.buttons.pressed")
                      : t("monitor.buttons.released")}
                  </Badge>
                </li>
              );
            })}
          </ul>
        )}
      </CardContent>
    </Card>
  );
}
