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
  const [selectedDay, setSelectedDay] = useState<WatchDayDto | null>(null);
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
          setSelectedDay(null);
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
  const activeDays = days.filter((day) => (day.totalMs ?? 0) > 0).length;
  const peakDay = days.reduce<WatchDayDto | null>(
    (current, day) =>
      current === null || (day.totalMs ?? 0) > (current.totalMs ?? 0) ? day : current,
    null,
  );

  const choose = (next: (typeof PRESETS)[number]): void => {
    setPreset(next);
    if (next !== "all") {
      setFrom(isoDaysAgo(Number(next)));
      setTo(new Date().toISOString().slice(0, 10));
    }
  };

  return (
    <section
      className="watch-metrics-panel flex shrink-0 flex-col gap-3 rounded-md border border-border-subtle bg-surface-1 p-3"
      aria-label={t("history.metrics.title")}
    >
      <div className="flex flex-wrap items-end justify-between gap-3">
        <div className="flex flex-col gap-1">
          <span className="text-caption text-text-tertiary">{t("history.metrics.title")}</span>
          <span className="text-title text-text-primary">{duration(total, translate)}</span>
          <span className="text-caption text-text-tertiary">
            {t("history.metrics.activeDays", { count: activeDays })}
          </span>
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
        <div className="watch-metrics-chart-wrap">
          <div aria-live="polite" className="watch-metrics-detail">
            {selectedDay === null ? (
              <span>{t("history.metrics.hoverHint")}</span>
            ) : (
              <>
                <strong>{selectedDay.day}</strong>
                <span>{duration(selectedDay.totalMs ?? 0, translate)}</span>
              </>
            )}
          </div>
          <svg
            aria-label={t("history.metrics.chart")}
            className="watch-metrics-chart"
            role="img"
            viewBox="0 0 720 220"
            preserveAspectRatio="none"
          >
            <defs>
              <linearGradient id="watch-area" x1="0" x2="0" y1="0" y2="1">
                <stop offset="0%" stopColor="var(--accent)" stopOpacity="0.32" />
                <stop offset="100%" stopColor="var(--accent)" stopOpacity="0.02" />
              </linearGradient>
            </defs>
            {[35, 88, 141, 194].map((y) => (
              <line
                className="watch-metrics-grid-line"
                key={y}
                x1="0"
                x2="720"
                y1={y}
                y2={y}
              />
            ))}
            {(() => {
              const points = days.map((day, index) => {
                const x = days.length === 1 ? 360 : (index / (days.length - 1)) * 720;
                const y = 194 - ((day.totalMs ?? 0) / peak) * 159;
                return { day, x, y };
              });
              const line = points.map(({ x, y }) => `${x},${y}`).join(" ");
              const area = `0,194 ${line} 720,194`;
              return (
                <>
                  <polygon className="watch-metrics-area" points={area} />
                  <polyline className="watch-metrics-line" points={line} />
                  {points.map(({ day, x, y }) => (
                    <circle
                      aria-label={`${day.day}: ${duration(day.totalMs ?? 0, translate)}`}
                      className="watch-metrics-point"
                      cx={x}
                      cy={y}
                      key={day.day}
                      onBlur={() => {
                        setSelectedDay(null);
                      }}
                      onFocus={() => {
                        setSelectedDay(day);
                      }}
                      onMouseEnter={() => {
                        setSelectedDay(day);
                      }}
                      onMouseLeave={() => {
                        setSelectedDay(null);
                      }}
                      r="3.5"
                      tabIndex={0}
                    >
                      <title>{`${day.day}: ${duration(day.totalMs ?? 0, translate)}`}</title>
                    </circle>
                  ))}
                </>
              );
            })()}
          </svg>
          <div className="watch-metrics-labels" aria-hidden="true">
            <span>{days[0]?.day}</span>
            <span>{days[Math.floor(days.length / 2)]?.day}</span>
            <span>{days[days.length - 1]?.day}</span>
          </div>
        </div>
      )}
      {peakDay === null ? null : (
        <div className="watch-metrics-footer">
          <span>
            {t("history.metrics.peak", {
              day: peakDay.day,
              duration: duration(peakDay.totalMs ?? 0, translate),
            })}
          </span>
          <span>
            {t("history.metrics.dailyAverage", {
              duration: duration(activeDays === 0 ? 0 : total / activeDays, translate),
            })}
          </span>
        </div>
      )}
    </section>
  );
}
