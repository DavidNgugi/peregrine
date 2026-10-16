/**
 * The Downloads screen: the transfer queue, and the three moves a row offers.
 *
 * It is a projection of one core read and nothing else — no sorting, no arithmetic on eligibility,
 * no progress the core did not report. What it does own is the drawing: a table of few rows, a bar
 * per row, and the confirmation a destructive move requires (AGENTS R22).
 *
 * **Remove is confirmed, and says what it does not do.** The job leaves the queue; the bytes already
 * written stay on disk, because deleting a library file is a different, reversible operation and
 * saying so here is what keeps a person from expecting one from the other.
 */

import {
  Button,
  Dialog,
  EmptyState,
  ErrorState,
  Progress,
  Screen,
  Section,
  Table,
} from "@peregrine/ui";
import type { TableColumn } from "@peregrine/ui";
import type { DownloadJobDto } from "@peregrine/ipc-client";
import type { ReactElement } from "react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { messageOf } from "../../../lib/ipc-message.js";
import { useDownloadActions } from "../hooks/useDownloadActions.js";
import type { Downloads } from "../hooks/useDownloads.js";

/** What `useTranslation` hands a component. */
type Translate = ReturnType<typeof useTranslation>["t"];

/** A state from which no move is left to make. */
const FINISHED_STATES: ReadonlySet<string> = new Set([
  "completed",
  "failed",
  "blocked",
  "removed",
]);

/** Bytes as a row shows them, in whole units. */
function sizeLabel(bytes: number, t: Translate): string {
  if (bytes <= 0) {
    return t("downloads.size.none");
  }
  const units = ["B", "KB", "MB", "GB", "TB"];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  const rounded = value < 10 ? Math.round(value * 10) / 10 : Math.round(value);
  return t("downloads.size.value", { value: rounded, unit: units[unit] ?? "B" });
}

/** A rate as a row shows it, or the em dash when nothing has moved yet. */
function rateLabel(bytesPerSecond: number, t: Translate): string {
  if (bytesPerSecond <= 0) {
    return t("downloads.none");
  }
  return t("downloads.rate.value", { value: sizeLabel(bytesPerSecond, t) });
}

/** A remaining time in the largest whole unit that reads. */
function etaLabel(seconds: number | null, t: Translate): string {
  if (seconds === null || seconds <= 0) {
    return t("downloads.none");
  }
  if (seconds < 60) {
    return t("downloads.eta.seconds", { value: seconds });
  }
  if (seconds < 3_600) {
    return t("downloads.eta.minutes", { value: Math.round(seconds / 60) });
  }
  return t("downloads.eta.hours", { value: Math.round(seconds / 3_600) });
}

/** The Downloads screen. */
export function DownloadsScreen({ downloads }: { readonly downloads: Downloads }): ReactElement {
  const { t } = useTranslation();
  const actions = useDownloadActions(downloads.refresh);
  const [removing, setRemoving] = useState<DownloadJobDto | null>(null);

  const confirmRemove = (): void => {
    const target = removing;
    setRemoving(null);
    if (target !== null) {
      void actions.remove(target.id);
    }
  };

  const columns: readonly TableColumn<DownloadJobDto>[] = [
    {
      id: "title",
      header: t("downloads.column.title"),
      cell: (row) => row.title ?? row.releaseTitle,
    },
    {
      id: "state",
      header: t("downloads.column.state"),
      cell: (row) => t(`downloads.state.${row.state}`),
    },
    {
      id: "reason",
      header: t("downloads.column.reason"),
      // Only a blocked row has findings, and they are the whole reason it is not downloading. The
      // report is the one the gate persisted, read back rather than recomputed (invariant I4).
      cell: (row) =>
        row.state === "blocked" && row.findings.length > 0 ? (
          <ul className="flex flex-col gap-1 text-caption text-text-tertiary">
            {row.findings.map((finding, index) => (
              <li key={`${finding.reason}-${String(index)}`}>
                {t(`downloads.finding.${finding.reason}`, { defaultValue: finding.reason })}
                {finding.entry === null ? "" : ` — ${finding.entry}`}
              </li>
            ))}
          </ul>
        ) : (
          t("downloads.none")
        ),
    },
    {
      id: "progress",
      header: t("downloads.column.progress"),
      cell: (row) => (
        <div className="flex items-center gap-2">
          <Progress
            label={t("downloads.progress.label", { title: row.title ?? row.releaseTitle })}
            value={Math.min(100, Math.max(0, row.progressPermille / 10))}
          />
          <span className="text-text-tertiary">{Math.round(row.progressPermille / 10)}%</span>
        </div>
      ),
    },
    {
      id: "size",
      header: t("downloads.column.size"),
      numeric: true,
      cell: (row) =>
        row.reportedSizeBytes === null
          ? sizeLabel(row.downloadedBytes, t)
          : `${sizeLabel(row.downloadedBytes, t)} / ${sizeLabel(row.reportedSizeBytes, t)}`,
    },
    {
      id: "rate",
      header: t("downloads.column.rate"),
      numeric: true,
      cell: (row) => rateLabel(row.downloadRateBps, t),
    },
    {
      id: "eta",
      header: t("downloads.column.eta"),
      numeric: true,
      cell: (row) => etaLabel(row.etaSeconds, t),
    },
    {
      id: "peers",
      header: t("downloads.column.peers"),
      numeric: true,
      cell: (row) => `${String(row.peersConnected)} / ${String(row.peersSwarm)}`,
    },
    {
      id: "actions",
      header: t("downloads.column.actions"),
      cell: (row) => {
        const busy = actions.busyId === row.id;
        const paused = row.state === "paused";
        return (
          <div className="flex items-center gap-2">
            {!paused && !FINISHED_STATES.has(row.state) ? (
              <Button
                disabled={busy}
                onClick={() => {
                  void actions.pause(row.id);
                }}
                size="sm"
                variant="ghost"
              >
                {t("downloads.action.pause")}
              </Button>
            ) : null}
            {paused ? (
              <Button
                disabled={busy}
                onClick={() => {
                  void actions.resume(row.id);
                }}
                size="sm"
                variant="ghost"
              >
                {t("downloads.action.resume")}
              </Button>
            ) : null}
            <Button
              disabled={busy}
              onClick={() => {
                setRemoving(row);
              }}
              size="sm"
              variant="danger"
            >
              {t("downloads.action.remove")}
            </Button>
          </div>
        );
      },
    },
  ];

  return (
    <Screen subtitle={t("downloads.subtitle")} title={t("downloads.title")}>
      <Section description={t("downloads.description")} title={t("downloads.section")}>
        {downloads.failed ? (
          <ErrorState
            action={
              <Button
                onClick={() => {
                  void downloads.refresh();
                }}
                variant="secondary"
              >
                {t("shell.error.retry")}
              </Button>
            }
            body={t("downloads.error.body")}
            title={t("downloads.error.title")}
          />
        ) : downloads.loading && downloads.downloads.length === 0 ? null : downloads.downloads
            .length === 0 ? (
          <EmptyState body={t("downloads.empty.body")} size="page" title={t("downloads.empty.title")} />
        ) : (
          <>
            {actions.error === null ? null : (
              <p className="text-caption text-state-danger" role="status">
                {messageOf(actions.error)}
              </p>
            )}
            <Table columns={columns} rowKey={(row) => row.id} rows={downloads.downloads} />
          </>
        )}
      </Section>

      {removing === null ? null : (
        <Dialog
          closeLabel={t("downloads.remove.cancel")}
          description={t("downloads.remove.body")}
          onClose={() => {
            setRemoving(null);
          }}
          title={t("downloads.remove.title")}
        >
          <div className="flex justify-end gap-2">
            <Button
              onClick={() => {
                setRemoving(null);
              }}
              variant="ghost"
            >
              {t("downloads.remove.cancel")}
            </Button>
            <Button onClick={confirmRemove} variant="danger">
              {t("downloads.remove.confirm")}
            </Button>
          </div>
        </Dialog>
      )}
    </Screen>
  );
}
