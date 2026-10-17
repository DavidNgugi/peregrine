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
  Link,
  Progress,
  Screen,
  Section,
  Table,
} from "@peregrine/ui";
import type { TableColumn } from "@peregrine/ui";
import type { DeletionPlanDto, DownloadJobDto } from "@peregrine/ipc-client";
import type { ReactElement } from "react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { routePath, type Route } from "../../../app/routes.js";
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

/** A state nothing may leave, so the row offers no move at all.
 *
 * `blocked` is deliberately absent: the domain gives it a dismissal edge, so Remove is the move
 * that clears a block rather than a terminal no-op. */
const TERMINAL_STATES: ReadonlySet<string> = new Set(["completed", "failed", "removed"]);

/** A finished entry the user can clear out of the list.
 *
 * `completed` is here because a completed payload is no longer seeding: the core stops the torrent
 * when it finishes, so clearing the row removes a record and changes nothing on the swarm. */
const CLEARABLE_STATES: ReadonlySet<string> = new Set([
  "completed",
  "failed",
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
export function DownloadsScreen({
  downloads,
  onNavigate,
  onBack,
  backLabel,
}: {
  readonly downloads: Downloads;
  /** Opens the title a row is filling, through the same route a tile uses. */
  readonly onNavigate: (route: Route) => void;
  /** The shell's back control, present only when there is somewhere behind this screen. */
  readonly onBack?: (() => void) | undefined;
  readonly backLabel?: string | undefined;
}): ReactElement {
  const { t } = useTranslation();
  const actions = useDownloadActions(downloads.refresh);
  // One dialog serves the three destructive moves: removing a running job, dismissing a finished
  // one, and clearing every finished one. `null` is the closed state.
  const [confirming, setConfirming] = useState<
    | { kind: "remove" | "dismiss"; row: DownloadJobDto }
    | { kind: "clear" }
    | { kind: "delete"; row: DownloadJobDto; plan: DeletionPlanDto }
    | null
  >(null);

  const confirm = (): void => {
    const target = confirming;
    setConfirming(null);
    if (target === null) {
      return;
    }
    if (target.kind === "clear") {
      void actions.clearFinished();
    } else if (target.kind === "delete") {
      void actions.deleteFiles(target.row.id);
    } else if (target.kind === "remove") {
      void actions.remove(target.row.id);
    } else {
      void actions.dismiss(target.row.id);
    }
  };

  const hasClearable = downloads.downloads.some((row) => CLEARABLE_STATES.has(row.state));

  const columns: readonly TableColumn<DownloadJobDto>[] = [
    {
      id: "title",
      header: t("downloads.column.title"),
      // **The title a transfer fills is a link, and the release name is what is left when the
      // library cannot name one.** A row whose title resolves carries the kind the route needs, so
      // opening it is the same move a poster makes; a row that resolves nothing stays text rather
      // than a link to a guessed route.
      cell: (row) => {
        const label = row.title ?? row.releaseTitle;
        if (row.titleKind === null) {
          return label;
        }
        const route: Route = { id: row.titleId, kind: "title", titleKind: row.titleKind };
        return (
          <Link
            href={routePath(route)}
            onNavigate={() => {
              onNavigate(route);
            }}
          >
            {label}
          </Link>
        );
      },
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
            {CLEARABLE_STATES.has(row.state) ? (
              <Button
                disabled={busy || actions.clearing}
                onClick={() => {
                  setConfirming({ kind: "dismiss", row });
                }}
                size="sm"
                variant="ghost"
              >
                {t("downloads.action.dismiss")}
              </Button>
            ) : null}
            {TERMINAL_STATES.has(row.state) ? null : (
              <Button
                disabled={busy}
                onClick={() => {
                  setConfirming({ kind: "remove", row });
                }}
                size="sm"
                variant="danger"
              >
                {t("downloads.action.remove")}
              </Button>
            )}
            {row.downloadedBytes > 0 && row.state !== "removed" ? (
              <Button
                disabled={busy || actions.planningId === row.id}
                onClick={() => {
                  void actions.planDeletion(row.id).then((plan) => {
                    if (plan !== null) {
                      setConfirming({ kind: "delete", row, plan });
                    }
                  });
                }}
                size="sm"
                variant="danger"
              >
                {t("downloads.action.delete")}
              </Button>
            ) : null}
          </div>
        );
      },
    },
  ];

  const copy =
    confirming === null
      ? null
      : confirming.kind === "clear"
        ? {
            title: t("downloads.clear.title"),
            body: t("downloads.clear.body"),
            confirm: t("downloads.clear.confirm"),
            cancel: t("downloads.clear.cancel"),
          }
        : confirming.kind === "dismiss"
          ? {
              title: t("downloads.dismiss.title"),
              body: t("downloads.dismiss.body"),
              confirm: t("downloads.dismiss.confirm"),
              cancel: t("downloads.dismiss.cancel"),
            }
          : confirming.kind === "delete"
            ? {
                title: t("downloads.delete.title"),
                body: t("downloads.delete.body", {
                  files: confirming.plan.entries.length,
                  size: sizeLabel(confirming.plan.totalBytes, t),
                }),
                confirm: t("downloads.delete.confirm"),
                cancel: t("downloads.delete.cancel"),
              }
            : {
                title: t("downloads.remove.title"),
                body: t("downloads.remove.body"),
                confirm: t("downloads.remove.confirm"),
                cancel: t("downloads.remove.cancel"),
              };

  return (
    <Screen backLabel={backLabel} onBack={onBack} subtitle={t("downloads.subtitle")} title={t("downloads.title")}>
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
            <div className="flex justify-end">
              <Button
                disabled={!hasClearable || actions.clearing}
                onClick={() => {
                  setConfirming({ kind: "clear" });
                }}
                size="sm"
                variant="secondary"
              >
                {t("downloads.clear.action")}
              </Button>
            </div>
            <Table columns={columns} rowKey={(row) => row.id} rows={downloads.downloads} />
          </>
        )}
      </Section>

      {copy === null ? null : (
        <Dialog
          closeLabel={copy.cancel}
          description={copy.body}
          onClose={() => {
            setConfirming(null);
          }}
          title={copy.title}
        >
          {confirming !== null && confirming.kind === "delete" ? (
            <ul className="mb-3 flex max-h-60 flex-col gap-1 overflow-auto text-caption text-text-tertiary">
              {confirming.plan.entries.map((entry) => (
                <li className="truncate" key={entry.path} title={entry.path}>
                  {entry.path}
                </li>
              ))}
            </ul>
          ) : null}
          <div className="flex justify-end gap-2">
            <Button
              onClick={() => {
                setConfirming(null);
              }}
              variant="ghost"
            >
              {copy.cancel}
            </Button>
            <Button onClick={confirm} variant="danger">
              {copy.confirm}
            </Button>
          </div>
        </Dialog>
      )}
    </Screen>
  );
}
