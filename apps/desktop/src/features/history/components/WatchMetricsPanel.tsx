import { Button } from "@peregrine/ui";
import { useEffect, useState, type ReactElement } from "react";
import { useTranslation } from "react-i18next";
import { watchMetrics, type WatchDayDto } from "@peregrine/ipc-client";

const PRESETS = ["7", "30", "90", "all"] as const;

function isoDaysAgo(days: number): string {
  const date = new Date();
  date.setDate(date.getDate() - days);
  return date.toISOString().slice(0, 10);
}

function duration(
  ms: number,
  t: (key: string, values?: Record<string, unknown>) => string,
): string {
  if (ms < 60_000) return t("history.metrics.underMinute");
  const minutes = Math.round(ms / 60_000);
  if (minutes < 60) return t("history.metrics.minutes", { count: minutes });
  const hours = Math.floor(minutes / 60);
  return t("history.metrics.hours", { hours, minutes: minutes % 60 });
}

/** Total watch time and a daily bar graph, bounded by a preset or two dates. */
export function WatchMetricsPanel(): ReactElement {
  const { t } = useTranslation();
  const translate = t as (key: string, values?: Record<string, unknown>) => string;
  const [preset, setPreset] = useState<(typeof PRESETS)[number]>("30");
  const [from, setFrom] = useState(isoDaysAgo(30));
  const [to, setTo] = useState(new Date().toISOString().slice(0, 10));
  const [days, setDays] = useState<readonly WatchDayDto[]>([]);
  const [failed, setFailed] = useState(false);

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const answer = await watchMetrics(
          preset === "all" ? undefined : new Date(`${from}T00:00:00`).getTime(),
          preset === "all" ? undefined : new Date(`${to}T23:59:59`).getTime(),
        );
        if (!cancelled) {
          setDays(answer);
          setFailed(false);
        }
      } catch {
        if (!cancelled) setFailed(true);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [from, to, preset]);

  const total = days.reduce((sum, day) => sum + (day.totalMs ?? 0), 0);
  const peak = Math.max(1, ...days.map((day) => day.totalMs ?? 0));

  const choose = (next: (typeof PRESETS)[number]): void => {
    setPreset(next);
    if (next !== "all") {
      setFrom(isoDaysAgo(Number(next)));
      setTo(new Date().toISOString().slice(0, 10));
    }
  };

  return (
    <section
      className="flex flex-col gap-3 rounded-md border border-border-subtle bg-surface-1 p-3"
      aria-label={t("history.metrics.title")}
    >
      <div className="flex flex-wrap items-end justify-between gap-3">
        <div className="flex flex-col">
          <span className="text-caption text-text-tertiary">{t("history.metrics.title")}</span>
          <span className="text-title text-text-primary">{duration(total, translate)}</span>
        </div>
        <div className="flex flex-wrap items-center gap-2">
          {PRESETS.map((entry) => (
            <Button
              key={entry}
              onClick={() => {
                choose(entry);
              }}
              size="sm"
              variant={preset === entry ? "secondary" : "ghost"}
            >
              {t(`history.metrics.preset.${entry}`)}
            </Button>
          ))}
          <label className="flex items-center gap-1 text-caption text-text-tertiary">
            <input
              aria-label={t("history.metrics.from")}
              className="rounded-sm border border-border-subtle bg-surface-2 px-2 py-1 text-caption"
              onChange={(event) => {
                setPreset("all");
                setFrom(event.currentTarget.value);
              }}
              type="date"
              value={from}
            />
            <span>–</span>
            <input
              aria-label={t("history.metrics.to")}
              className="rounded-sm border border-border-subtle bg-surface-2 px-2 py-1 text-caption"
              onChange={(event) => {
                setPreset("all");
                setTo(event.currentTarget.value);
              }}
              type="date"
              value={to}
            />
          </label>
        </div>
      </div>
      {failed ? (
        <p className="text-caption text-state-danger" role="alert">
          {t("history.metrics.failed")}
        </p>
      ) : null}
      {days.length === 0 ? null : (
        <svg
          aria-label={t("history.metrics.chart")}
          className="h-24 w-full text-accent"
          role="img"
          viewBox={`0 0 ${Math.max(1, days.length * 12)} 100`}
          preserveAspectRatio="none"
        >
          {days.map((day, index) => {
            const height = Math.max(1, ((day.totalMs ?? 0) / peak) * 96);
            return (
              <rect
                key={day.day}
                fill="currentColor"
                height={height}
                opacity={0.55 + (height / 100) * 0.45}
                rx={2}
                width={8}
                x={index * 12 + 2}
                y={100 - height}
              >
                <title>{`${day.day}: ${duration(day.totalMs ?? 0, translate)}`}</title>
              </rect>
            );
          })}
        </svg>
      )}
    </section>
  );
}
