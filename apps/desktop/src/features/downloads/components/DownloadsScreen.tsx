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
  Icon,
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
import { downloadPercentOf } from "../../../lib/download-status.js";
import { messageOf } from "../../../lib/ipc-message.js";
import { imageUrl } from "../../../lib/media.js";
import { useDownloadActions } from "../hooks/useDownloadActions.js";
import { downloadTree, type QueueLine, type SeasonLine } from "./download-tree.js";
import { ScheduledTransfers } from "./ScheduledTransfers.js";
import type { Downloads } from "../hooks/useDownloads.js";

/** What `useTranslation` hands a component. */
type Translate = ReturnType<typeof useTranslation>["t"];

/** A state nothing may leave, so the row offers no move at all.
 *
 * `blocked` is deliberately absent: the domain gives it a dismissal edge, so Remove is the move
 * that clears a block rather than a terminal no-op. */
const TERMINAL_STATES: ReadonlySet<string> = new Set(["completed", "failed", "removed"]);

/** A finished entry the user can clear out of the list.
 *
 * `completed` is here because a completed payload is no longer seeding: the core stops the torrent
 * when it finishes, so clearing the row removes a record and changes nothing on the swarm. */
const CLEARABLE_STATES: ReadonlySet<string> = new Set(["completed", "failed", "removed"]);

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
  // One set of season keys, so collapsing one season leaves every other disclosure where it was.
  // Expanded is the default, and the set holds the exceptions.
  const [collapsedSeasons, setCollapsedSeasons] = useState<ReadonlySet<string>>(() => new Set());

  const confirm = async (): Promise<void> => {
    const target = confirming;
    if (target === null) {
      return;
    }
    if (target.kind === "clear") {
      await actions.clearFinished();
    } else if (target.kind === "delete") {
      await actions.deleteFiles(target.row.id);
    } else if (target.kind === "remove") {
      await actions.remove(target.row.id);
    } else {
      await actions.dismiss(target.row.id);
    }
    setConfirming(null);
  };

  const hasClearable = downloads.downloads.some((row) => CLEARABLE_STATES.has(row.state));
  const lines = downloadTree(downloads.downloads, collapsedSeasons);

  const toggleSeason = (key: string): void => {
    setCollapsedSeasons((current) => {
      const next = new Set(current);
      if (next.has(key)) {
        next.delete(key);
      } else {
        next.add(key);
      }
      return next;
    });
  };

  const jobColumns: readonly TableColumn<DownloadJobDto>[] = [
    {
      id: "title",
      header: t("downloads.column.title"),
      // **The title a transfer fills is a link, and the release name is what is left when the
      // library cannot name one.** A row whose title resolves carries the kind the route needs, so
      // opening it is the same move a poster makes; a row that resolves nothing stays text rather
      // than a link to a guessed route.
      cell: (row) => {
        const label = row.title ?? row.releaseTitle;
        const poster = imageUrl(row.poster);
        // The thumbnail is decorative: the label beside it is the cell's text and the link's name,
        // so an alt here would make a screen reader say the title twice.
        const content = (
          <span className="flex items-center gap-2">
            <span className="block h-10 w-7 shrink-0 overflow-hidden rounded-sm border border-border-subtle bg-surface-2">
              {poster === null ? null : (
                <img
                  alt=""
                  aria-hidden="true"
                  className="size-full object-cover"
                  decoding="async"
                  loading="lazy"
                  src={poster}
                />
              )}
            </span>
            <span className="flex min-w-0 flex-col">
              <span className="truncate">{label}</span>
              {row.seasonNumber !== null && row.episodeNumber !== null ? (
                <span className="text-caption text-text-tertiary">
                  {t("downloads.episode", {
                    season: row.seasonNumber,
                    episode: String(row.episodeNumber).padStart(2, "0"),
                  })}
                </span>
              ) : null}
              <span className="download-protocol">
                {t(`downloads.protocol.${row.protocol}`, { defaultValue: row.protocol })}
              </span>
            </span>
          </span>
        );
        if (row.titleKind === null) {
          return content;
        }
        const route: Route = { id: row.titleId, kind: "title", titleKind: row.titleKind };
        return (
          <Link
            href={routePath(route)}
            onNavigate={() => {
              onNavigate(route);
            }}
          >
            {content}
          </Link>
        );
      },
    },
    {
      id: "protocol",
      header: t("downloads.column.protocol"),
      cell: (row) => t(`downloads.protocol.${row.protocol}`, { defaultValue: row.protocol }),
    },
    {
      id: "state",
      header: t("downloads.column.state"),
      cell: (row) => (
        <div className="download-state-cell">
          <span>
            {row.state === "failed" || row.stage === "transfer"
              ? t(`downloads.state.${row.state}`)
              : t(`downloads.stage.${row.stage}`, { defaultValue: row.stage })}
          </span>
          {row.state === "blocked" && row.findings.length > 0 ? (
            <span className="download-blocked-reason">
              {t(`downloads.finding.${row.findings[0]?.reason}`, {
                defaultValue: row.findings[0]?.reason,
              })}
            </span>
          ) : null}
        </div>
      ),
    },
    {
      id: "reason",
      header: t("downloads.column.reason"),
      // A blocked row's findings are the whole reason it is not downloading: the report is the one
      // the gate persisted, read back rather than recomputed (invariant I4). A failed row carries
      // the core's own sentence instead, which is where a stalled transfer says it found no peers.
      cell: (row) => {
        if (row.state === "blocked" && row.findings.length > 0) {
          return (
            <ul className="flex flex-col gap-1 text-caption text-text-tertiary">
              {row.findings.map((finding, index) => (
                <li key={`${finding.reason}-${String(index)}`}>
                  {t(`downloads.finding.${finding.reason}`, { defaultValue: finding.reason })}
                  {finding.entry === null ? "" : ` — ${finding.entry}`}
                </li>
              ))}
            </ul>
          );
        }
        if (row.state === "failed" && row.errorMessage !== null) {
          return <span className="text-caption text-text-tertiary">{row.errorMessage}</span>;
        }
        return t("downloads.none");
      },
    },
    {
      id: "progress",
      header: t("downloads.column.progress"),
      cell: (row) => {
        const percent = downloadPercentOf(row.progressPermille);
        return (
          <div className="download-progress-cell">
            <div className="flex items-center gap-2">
              <Progress
                label={t("downloads.progress.label", { title: row.title ?? row.releaseTitle })}
                value={percent}
              />
              <span className="text-text-tertiary">{percent}%</span>
            </div>
          </div>
        );
      },
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
      cell: (row) =>
        row.protocol === "usenet"
          ? t("downloads.none")
          : `${String(row.peersConnected)} / ${String(row.peersSwarm)}`,
    },
    {
      id: "actions",
      header: t("downloads.column.actions"),
      cell: (row) => {
        if (row.isPreflight) {
          return row.state === "failed" ? (
            <Button
              className="download-primary-action"
              disabled={actions.busyId === row.titleId}
              onClick={() => actions.retryAcquire(row.titleId)}
              size="sm"
              variant="secondary"
            >
              {t("downloads.action.retry")}
            </Button>
          ) : (
            <span className="text-caption text-text-tertiary">{t("downloads.none")}</span>
          );
        }
        const busy = actions.busyId === row.id;
        const paused = row.state === "paused";
        // **A metadata row is a handoff that has not finished, not a transfer to pause.** After a
        // restart it may hold no engine handle at all; Resume is what completes the handoff, and
        // Pause has no meaning there (the state machine refuses it).
        const resumable = paused || row.state === "metadata";
        const pausable = row.state === "downloading" || row.state === "seeding";
        const play: Route =
          row.seasonNumber !== null && row.episodeNumber !== null
            ? {
                kind: "player",
                id: row.titleId,
                season: row.seasonNumber,
                episode: row.episodeNumber,
                ...(row.titleKind === null ? {} : { titleKind: row.titleKind }),
              }
            : {
                kind: "player",
                id: row.titleId,
                ...(row.titleKind === null ? {} : { titleKind: row.titleKind }),
              };
        return (
          <div className="download-actions">
            {/* A job whose payload has named a media file is playable now, not when it completes:
                the engine serves the bytes the transfer already has. */}
            {row.hasMediaFile && row.state !== "failed" && row.state !== "removed" ? (
              <Link className="download-primary-action" href={routePath(play)}>
                {t("downloads.action.play")}
              </Link>
            ) : null}
            {row.hasMediaFile === false && resumable ? (
              <Button
                className="download-primary-action"
                disabled={busy}
                onClick={() => actions.resume(row.id)}
                size="sm"
                variant="secondary"
              >
                {t("downloads.action.resume")}
              </Button>
            ) : null}
            {row.hasMediaFile === false && pausable ? (
              <Button
                className="download-primary-action"
                disabled={busy}
                onClick={() => actions.pause(row.id)}
                size="sm"
                variant="secondary"
              >
                {t("downloads.action.pause")}
              </Button>
            ) : null}
            <details className="download-action-menu">
              <summary>{t("downloads.action.more")}</summary>
              <div className="download-action-popover">
                {/* **A playable row keeps Play as its primary action, so Pause and Resume live
                    here.** A payload that has already named a media file is the one a person is
                    most likely to want stopped, and hiding the move behind the same row's primary
                    Play would leave the transfer with no control at all. A row without a media
                    file draws the same move as its primary button instead, never both. */}
                {row.hasMediaFile && resumable ? (
                  <Button
                    disabled={busy}
                    onClick={() => actions.resume(row.id)}
                    size="sm"
                    variant="secondary"
                  >
                    {t("downloads.action.resume")}
                  </Button>
                ) : null}
                {row.hasMediaFile && pausable ? (
                  <Button
                    disabled={busy}
                    onClick={() => actions.pause(row.id)}
                    size="sm"
                    variant="secondary"
                  >
                    {t("downloads.action.pause")}
                  </Button>
                ) : null}
                {/* The core re-announces a stalled transfer on its own; this is the same move on
                    demand, for a person who does not want to wait out the grace period. */}
                {row.protocol === "torrent" &&
                (row.state === "downloading" ||
                  row.state === "seeding" ||
                  row.state === "metadata") ? (
                  <Button
                    disabled={busy}
                    onClick={() => actions.findPeers(row.id)}
                    size="sm"
                    variant="secondary"
                  >
                    {t("downloads.action.findPeers")}
                  </Button>
                ) : null}
                {row.protocol === "usenet" &&
                row.state === "blocked" &&
                row.findings.some((finding) => finding.reason === "nested_archive") ? (
                  <Button
                    disabled={busy}
                    onClick={() => actions.consent(row.id)}
                    size="sm"
                    variant="secondary"
                  >
                    {t("downloads.action.allowArchives")}
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
            </details>
          </div>
        );
      },
    },
  ];

  const seasonTitle = (line: SeasonLine): ReactElement => {
    const expanded = !collapsedSeasons.has(line.key);
    // A pack is named beside the episodes rather than instead of them: a season can have both
    // queued, and a header that dropped one would misstate what the queue is doing.
    const episodes = t("downloads.season.episodes", { count: line.episodeCount });
    const summary =
      line.packCount === 0
        ? episodes
        : line.episodeCount === 0
          ? t("downloads.season.pack", { count: line.packCount })
          : t("downloads.season.episodesAndPack", {
              count: line.packCount,
              episodes,
            });
    const label = t(expanded ? "downloads.season.collapse" : "downloads.season.expand", {
      season: line.seasonNumber,
      title: line.title,
    });
    return (
      <span className="flex min-w-0 items-center gap-2">
        {/* The button is the disclosure: `aria-expanded` is the state a screen reader announces,
            and Enter and Space are the browser's own. */}
        <button
          aria-expanded={expanded}
          aria-label={label}
          className="grid size-6 shrink-0 place-items-center rounded-sm text-text-secondary hover:bg-surface-2 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-focus-ring"
          onClick={() => {
            toggleSeason(line.key);
          }}
          title={label}
          type="button"
        >
          <Icon
            className={
              expanded
                ? "rotate-90 transition-transform motion-reduce:transition-none"
                : "transition-transform motion-reduce:transition-none"
            }
            name="chevron-right"
            size={16}
          />
        </button>
        <span className="flex min-w-0 flex-col">
          <span className="truncate font-medium text-text-primary">{line.title}</span>
          <span className="text-caption text-text-tertiary">
            {t("downloads.season.number", { number: line.seasonNumber })}
            {" · "}
            {summary}
          </span>
        </span>
      </span>
    );
  };

  const seasonProgress = (line: SeasonLine): ReactElement =>
    line.episodeCount === 0 && line.packCount === 0 ? (
      <span className="text-text-tertiary">{t("downloads.none")}</span>
    ) : (
      <div className="download-progress-cell">
        <div className="flex items-center gap-2">
          <Progress
            label={t("downloads.season.progress", {
              title: line.title,
              season: line.seasonNumber,
            })}
            value={line.percent}
          />
          <span className="text-text-tertiary">{line.percent}%</span>
        </div>
      </div>
    );

  // The existing per-row cells are reused below the header, so a row keeps every move it had;
  // the two season columns are the only cells the header replaces.
  const columns: readonly TableColumn<QueueLine>[] = jobColumns.map((column) => ({
    ...column,
    cell: (line) => {
      if (line.kind === "job") {
        const content = column.cell(line.job);
        return line.nested && column.id === "title" ? (
          <span className="block ps-6">{content}</span>
        ) : (
          content
        );
      }
      if (column.id === "title") {
        return seasonTitle(line);
      }
      if (column.id === "progress") {
        return seasonProgress(line);
      }
      return null;
    },
  }));

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
    <Screen
      backLabel={backLabel}
      onBack={onBack}
      subtitle={t("downloads.subtitle")}
      title={t("downloads.title")}
    >
      <ScheduledTransfers />

      <Section description={t("downloads.description")} title={t("downloads.section")}>
        {downloads.failed ? (
          <ErrorState
            action={
              <Button onClick={() => downloads.refresh()} variant="secondary">
                {t("shell.error.retry")}
              </Button>
            }
            body={t("downloads.error.body")}
            title={t("downloads.error.title")}
          />
        ) : downloads.loading && downloads.downloads.length === 0 ? null : downloads.downloads
            .length === 0 ? (
          <EmptyState
            body={t("downloads.empty.body")}
            size="page"
            title={t("downloads.empty.title")}
          />
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
            <Table
              className="downloads-table"
              columns={columns}
              rowKey={(line) => line.key}
              rows={lines}
            />
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
