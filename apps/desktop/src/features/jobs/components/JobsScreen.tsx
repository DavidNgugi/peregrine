import { Button } from "@peregrine/ui";
import type { ReactElement } from "react";
import { useTranslation } from "react-i18next";
import { findJournalRow, isTerminal, percentOf } from "../job-progress.js";
import { useJobs } from "../hooks/useJobs.js";

/** The number of steps the screen asks for: about three seconds of work. */
const STEPS = 300;

/**
 * The Phase 0 diagnostics screen: start a hello job, watch it, cancel it, and read what
 * the core recorded.
 *
 * It exists because `PLAN.md`'s Phase 0 exit criteria are "a hello job runs through the
 * scheduler with visible progress" and "logs tail in Diagnostics" — both of which need a
 * surface, not only a test (PLAN §8 item 1). Routing replaces the shell's screen switch
 * with PG-E7-01, and this screen becomes the Diagnostics page with PG-E25-05.
 */
export function JobsScreen(): ReactElement {
  const { t } = useTranslation();
  const { live, journal, health, logLines, error, starting, cancelling, refreshing } = useJobs();
  const { start, cancel, refresh, clearError } = useJobs();

  const terminal = isTerminal(live?.phase ?? null);
  const row = findJournalRow(journal, live);
  // The event stream coalesces, so a job that finished inside one tick never reported a
  // progress value: the journal row is the record, and it has one.
  const permille = terminal && row !== null ? row.progressPermille : (live?.progressPermille ?? 0);
  // One number for both the bar and the label, so the two cannot disagree.
  const percent =
    live === null ? 0 : Math.round(percentOf({ ...live, progressPermille: permille }));

  return (
    <section
      aria-labelledby="jobs-title"
      className="flex h-full flex-col gap-4 overflow-y-auto px-6 py-4"
    >
      <header className="flex items-baseline justify-between gap-4">
        <h1 className="text-title-1 text-text-primary" id="jobs-title">
          {t("jobs.title")}
        </h1>
        <p className="text-caption text-text-tertiary">{t("jobs.subtitle")}</p>
      </header>

      {error !== null ? (
        <div
          className="flex items-center justify-between gap-3 rounded-md border border-border-subtle bg-surface-2 px-3 py-2"
          role="alert"
        >
          <span className="text-body text-text-primary">
            {t(`ipc.error.${error.code.toLowerCase()}.title`, {
              defaultValue: error.message,
            })}
          </span>
          <Button onClick={clearError} size="sm" variant="ghost">
            {t("jobs.error.dismiss")}
          </Button>
        </div>
      ) : null}

      <div className="flex flex-wrap items-center gap-2">
        <Button
          disabled={starting || (live !== null && !terminal)}
          onClick={() => {
            void start(STEPS);
          }}
        >
          {t("jobs.action.start")}
        </Button>
        <Button
          disabled={cancelling || live === null || terminal}
          onClick={() => {
            void cancel();
          }}
          variant="secondary"
        >
          {t("jobs.action.cancel")}
        </Button>
        <Button
          disabled={refreshing}
          onClick={() => {
            void refresh();
          }}
          variant="ghost"
        >
          {t("jobs.action.refresh")}
        </Button>
        {health?.events.degraded === true ? (
          <span className="text-caption text-text-tertiary" role="status">
            {t("jobs.degraded", { count: health.events.dropped + health.events.failed })}
          </span>
        ) : null}
      </div>

      <div className="flex flex-col gap-1">
        <div
          aria-label={t("jobs.progress.label")}
          aria-valuemax={100}
          aria-valuemin={0}
          aria-valuenow={percent}
          className="h-2 w-full overflow-hidden rounded-full bg-surface-2"
          role="progressbar"
        >
          <div
            className="h-full bg-accent transition-[width] duration-200 ease-out motion-reduce:transition-none"
            style={{ width: `${String(percent)}%` }}
          />
        </div>
        <p className="text-caption text-text-secondary" role="status">
          {live === null
            ? t("jobs.progress.idle")
            : t("jobs.progress.value", { phase: live.phase ?? "queued", percent })}
        </p>
      </div>

      <section aria-labelledby="jobs-journal-title" className="flex flex-col gap-2">
        <h2 className="text-title-2 text-text-primary" id="jobs-journal-title">
          {t("jobs.journal.title")}
        </h2>
        {journal.length === 0 ? (
          <p className="text-body text-text-tertiary">{t("jobs.journal.empty")}</p>
        ) : (
          <table className="w-full border-collapse text-caption">
            <thead>
              <tr className="text-left text-text-tertiary">
                <th scope="col">{t("jobs.journal.id")}</th>
                <th scope="col">{t("jobs.journal.kind")}</th>
                <th scope="col">{t("jobs.journal.state")}</th>
                <th scope="col">{t("jobs.journal.progress")}</th>
                <th scope="col">{t("jobs.journal.attempts")}</th>
              </tr>
            </thead>
            <tbody>
              {journal.map((entry) => (
                <tr className="border-t border-border-subtle text-text-secondary" key={entry.id}>
                  <td>{entry.id}</td>
                  <td>{entry.kind}</td>
                  <td>{entry.state}</td>
                  <td>{`${String(entry.progressPermille / 10)}%`}</td>
                  <td>{`${String(entry.attempts)}/${String(entry.maxAttempts)}`}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </section>

      <section aria-labelledby="jobs-log-title" className="flex min-h-0 flex-col gap-2">
        <h2 className="text-title-2 text-text-primary" id="jobs-log-title">
          {t("jobs.log.title")}
        </h2>
        {logLines.length === 0 ? (
          <p className="text-body text-text-tertiary">{t("jobs.log.empty")}</p>
        ) : (
          <pre className="max-h-64 overflow-auto rounded-md bg-surface-2 p-3 text-caption text-text-secondary">
            {logLines.join("\n")}
          </pre>
        )}
      </section>
    </section>
  );
}
