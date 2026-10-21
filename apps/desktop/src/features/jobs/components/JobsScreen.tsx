import {
  Button,
  Dialog,
  EmptyState,
  ErrorState,
  Panel,
  Progress,
  Screen,
  Section,
  Table,
  type TableColumn,
} from "@peregrine/ui";
import type { JobRecord } from "@peregrine/ipc-client";
import { describeIpcError } from "@peregrine/ipc-client";
import type { ReactElement } from "react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { findJournalRow, isTerminal, percentOf } from "../job-progress.js";
import { useJobs } from "../hooks/useJobs.js";

/** The number of steps the screen asks for: about three seconds of work. */
const STEPS = 300;

/**
 * The Phase 0 diagnostics screen: start a hello job, watch it, cancel it, and read what the
 * core recorded.
 *
 * It exists because `PLAN.md`'s Phase 0 exit criteria are "a hello job runs through the
 * scheduler with visible progress" and "logs tail in Diagnostics" — both of which need a
 * surface, not only a test (PLAN §8 item 1). PG-E25-05 turns it into the full Diagnostics
 * page; PG-E7-02 replaces the shell's screen switch with a route to it.
 *
 * Every element is the kit's — `Screen` owns the heading and the scroll, `Section` owns the
 * two titled regions, `Table` owns the journal, `Progress` owns the bar, `Panel` owns the log
 * surface, and `EmptyState`/`ErrorState` own the two ways a region can have nothing to show.
 * What is left here is copy and state.
 */
export function JobsScreen({
  onBack,
  backLabel,
}: {
  /** The shell's back control, present only when there is somewhere behind this screen. */
  readonly onBack?: (() => void) | undefined;
  readonly backLabel?: string | undefined;
}): ReactElement {
  const { t } = useTranslation();
  const { live, journal, health, logLines, error, starting, cancelling, refreshing } = useJobs();
  const { start, cancel, refresh, clearError } = useJobs();
  const [confirmingCancel, setConfirmingCancel] = useState(false);

  const terminal = isTerminal(live?.phase ?? null);
  const row = findJournalRow(journal, live);
  // The event stream coalesces, so a job that finished inside one tick never reported a
  // progress value: the journal row is the record, and it has one.
  const permille = terminal && row !== null ? row.progressPermille : (live?.progressPermille ?? 0);
  // One number for both the bar and the label, so the two cannot disagree.
  const percent =
    live === null ? 0 : Math.round(percentOf({ ...live, progressPermille: permille }));

  // The core's structured error, through the shared mapping: the title says what happened
  // and the advice says what to do about it, which is the pair PLAN §8 item 4 asks for. The
  // raw `error.message` is written to the UI log by the store and never shown here, because
  // a message from the core can name a path (SECURITY.md T13).
  const errorCopy = error === null ? null : describeIpcError(error);

  const columns: readonly TableColumn<JobRecord>[] = [
    { id: "id", header: t("jobs.journal.id"), cell: (entry) => entry.id },
    { id: "kind", header: t("jobs.journal.kind"), cell: (entry) => entry.kind },
    { id: "state", header: t("jobs.journal.state"), cell: (entry) => entry.state },
    {
      id: "progress",
      header: t("jobs.journal.progress"),
      numeric: true,
      cell: (entry) => `${String(entry.progressPermille / 10)}%`,
    },
    {
      id: "attempts",
      header: t("jobs.journal.attempts"),
      numeric: true,
      cell: (entry) => `${String(entry.attempts)}/${String(entry.maxAttempts)}`,
    },
  ];

  return (
    <Screen
      backLabel={backLabel}
      onBack={onBack}
      subtitle={t("jobs.subtitle")}
      title={t("jobs.title")}
    >
      {errorCopy !== null ? (
        <ErrorState
          action={
            <Button onClick={clearError} size="sm" variant="ghost">
              {t("jobs.error.dismiss")}
            </Button>
          }
          body={t(errorCopy.adviceKey)}
          title={t(errorCopy.titleKey)}
        />
      ) : null}

      <div className="flex flex-wrap items-center justify-end gap-2">
        <Button
          disabled={starting || (live !== null && !terminal)}
          onClick={() => start(STEPS)}
          variant="primary"
        >
          {t("jobs.action.start")}
        </Button>
        <Button
          disabled={cancelling || live === null || terminal}
          onClick={() => {
            setConfirmingCancel(true);
          }}
          variant="danger"
        >
          {t("jobs.action.cancel")}
        </Button>
        <Button disabled={refreshing} onClick={() => refresh()} variant="secondary">
          {t("jobs.action.refresh")}
        </Button>
        {health?.events.degraded === true ? (
          <p className="text-caption text-text-tertiary" role="status">
            {t("jobs.degraded", { count: health.events.dropped + health.events.failed })}
          </p>
        ) : null}
      </div>

      {confirmingCancel ? (
        <Dialog
          closeLabel={t("jobs.cancelConfirm.cancel")}
          description={t("jobs.cancelConfirm.body")}
          onClose={() => {
            setConfirmingCancel(false);
          }}
          title={t("jobs.cancelConfirm.title")}
        >
          <div className="flex justify-end gap-2">
            <Button
              onClick={() => {
                setConfirmingCancel(false);
              }}
              size="sm"
              variant="ghost"
            >
              {t("jobs.cancelConfirm.cancel")}
            </Button>
            <Button
              onClick={() =>
                cancel().finally(() => {
                  setConfirmingCancel(false);
                })
              }
              size="sm"
              variant="danger"
            >
              {t("jobs.cancelConfirm.confirm")}
            </Button>
          </div>
        </Dialog>
      ) : null}

      <div className="flex flex-col gap-1">
        <Progress label={t("jobs.progress.label")} value={percent} />
        <p className="text-caption text-text-secondary" role="status">
          {live === null
            ? t("jobs.progress.idle")
            : t("jobs.progress.value", { phase: live.phase ?? "queued", percent })}
        </p>
      </div>

      <Section title={t("jobs.journal.title")}>
        {journal.length === 0 ? (
          <EmptyState title={t("jobs.journal.empty")} />
        ) : (
          <Table columns={columns} rowKey={(entry) => entry.id} rows={journal} />
        )}
      </Section>

      <Section title={t("jobs.log.title")}>
        {logLines.length === 0 ? (
          <EmptyState title={t("jobs.log.empty")} />
        ) : (
          <Panel>
            {/* A `<pre>` is the element for a log: the line breaks are the content. */}
            <pre className="max-h-64 overflow-auto p-3 font-mono text-caption text-text-secondary">
              {logLines.join("\n")}
            </pre>
          </Panel>
        )}
      </Section>
    </Screen>
  );
}
