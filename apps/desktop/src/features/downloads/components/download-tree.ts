/**
 * The queue as a tree: one header per season, with that season's job rows beneath it.
 *
 * **Grouping is drawing, not a rule.** The core owns the queue's states and its order; this folds
 * rows that already share a title and a season under one disclosure so that a season of eight
 * episodes reads as one line rather than eight. The aggregate is `season-download.ts`'s rule
 * applied to the queue's own rows: floored from each episode's permille, and 100 only when every
 * episode is done.
 *
 * **A live season pack is the season's own transfer.** A row with a season and no episode, in a
 * state the queue is still moving, is counted as a pack rather than as an episode: a lone pack's
 * header says "1 pack" and takes its percent from the pack itself, because "0 episodes" would
 * deny the transfer that is running. When episode rows sit beside it the header states both — the
 * episodes keep the floored aggregate and the pack is named alongside, never silently dropped. A
 * failed, removed, or finished pack is not live and leaves the header as it was.
 */

import type { DownloadJobDto } from "@peregrine/ipc-client";

import { downloadPercentOf, downloadStatusOf } from "../../../lib/download-status.js";

/** One season's disclosure, and the job rows it holds when it is open. */
export interface SeasonLine {
  readonly kind: "season";
  /** Stable across a refresh and unique across titles, so one season's state cannot toggle another's. */
  readonly key: string;
  readonly seasonNumber: number;
  /** The library's name for the title, or the release name when it cannot name one. */
  readonly title: string;
  /** Distinct episodes, because a retry beside its replacement is still one episode. */
  readonly episodeCount: number;
  /** Live rows that cover the whole season rather than one episode of it. */
  readonly packCount: number;
  /** The pack's own completion when it alone covers the season, otherwise the episodes' aggregate. */
  readonly percent: number;
  /** Every row in the season, including a season pack and any duplicate, for the rows beneath. */
  readonly members: readonly DownloadJobDto[];
}

/** One job row: either a title-level transfer or a member of the season above it. */
export interface JobLine {
  readonly kind: "job";
  readonly key: string;
  readonly job: DownloadJobDto;
  /** The row sits under a season header, so its title cell is indented. */
  readonly nested: boolean;
}

/** One visible line of the queue. */
export type QueueLine = SeasonLine | JobLine;

/** The key a season's collapse state is held under. */
function seasonLineKey(titleId: number, seasonNumber: number): string {
  return `season:${String(titleId)}:${String(seasonNumber)}`;
}

/** One row per episode number, first occurrence wins because the core sends newest first. */
function distinctEpisodes(members: readonly DownloadJobDto[]): readonly DownloadJobDto[] {
  const seen = new Set<number>();
  const episodes: DownloadJobDto[] = [];
  for (const member of members) {
    const number = member.episodeNumber;
    if (number === null || seen.has(number)) {
      continue;
    }
    seen.add(number);
    episodes.push(member);
  }
  return episodes;
}

/**
 * A season's aggregate, with the same floor `season-download.ts` uses.
 *
 * The clamp to 99 is the second half of that rule: a payload that has every byte can still be
 * waiting on its state transition, and a season is finished only when each episode's state says
 * so — never because an average rounded up.
 */
function seasonPercent(episodes: readonly DownloadJobDto[]): number {
  if (episodes.length === 0) {
    return 0;
  }
  let total = 0;
  let done = 0;
  for (const episode of episodes) {
    total += downloadPercentOf(episode.progressPermille);
    if (downloadStatusOf(episode.state) === "done") {
      done += 1;
    }
  }
  return done === episodes.length ? 100 : Math.min(99, Math.floor(total / episodes.length));
}

/** A row that covers a whole season rather than one episode, and is still under way. */
function isLivePack(row: DownloadJobDto): boolean {
  return row.episodeNumber === null && downloadStatusOf(row.state) === "active";
}

/**
 * A live pack's own completion.
 *
 * The same choice `season-download.ts` makes: a running row wins over a queued retry beside it,
 * and the furthest along wins within that, so the queue header and the title control name the
 * same pack's number.
 */
function packPercent(packs: readonly DownloadJobDto[]): number {
  let chosen: DownloadJobDto | null = null;
  for (const pack of packs) {
    if (chosen === null) {
      chosen = pack;
      continue;
    }
    const moving = pack.state !== "queued";
    const chosenMoving = chosen.state !== "queued";
    if (
      (moving && !chosenMoving) ||
      (moving === chosenMoving && pack.progressPermille > chosen.progressPermille)
    ) {
      chosen = pack;
    }
  }
  return chosen === null ? 0 : downloadPercentOf(chosen.progressPermille);
}

/**
 * The queue's visible lines, with a collapsed season's members left out.
 *
 * A row without a season has no group to belong to and stays where the core put it. A season is
 * emitted at the position of its first row, so the queue's newest-first order survives grouping.
 */
export function downloadTree(
  rows: readonly DownloadJobDto[],
  collapsed: ReadonlySet<string>,
): readonly QueueLine[] {
  const seasons = new Map<string, DownloadJobDto[]>();
  for (const row of rows) {
    if (row.seasonNumber === null) {
      continue;
    }
    const key = seasonLineKey(row.titleId, row.seasonNumber);
    const held = seasons.get(key);
    if (held === undefined) {
      seasons.set(key, [row]);
    } else {
      held.push(row);
    }
  }

  const lines: QueueLine[] = [];
  const emitted = new Set<string>();
  for (const row of rows) {
    if (row.seasonNumber === null) {
      lines.push({ kind: "job", key: `job:${String(row.id)}`, job: row, nested: false });
      continue;
    }
    const key = seasonLineKey(row.titleId, row.seasonNumber);
    if (emitted.has(key)) {
      continue;
    }
    emitted.add(key);
    const members = seasons.get(key);
    const first = members?.[0];
    if (members === undefined || first === undefined) {
      continue;
    }
    const episodes = distinctEpisodes(members);
    const packs = members.filter(isLivePack);
    lines.push({
      kind: "season",
      key,
      seasonNumber: row.seasonNumber,
      title: members.find((member) => member.title !== null)?.title ?? first.releaseTitle,
      episodeCount: episodes.length,
      packCount: packs.length,
      // The pack is the season's only row when no episode is queued, so its own percent is the
      // season's; with episodes present the header keeps the rule those rows already follow.
      percent: episodes.length === 0 ? packPercent(packs) : seasonPercent(episodes),
      members,
    });
    if (collapsed.has(key)) {
      continue;
    }
    for (const member of members) {
      lines.push({ kind: "job", key: `job:${String(member.id)}`, job: member, nested: true });
    }
  }
  return lines;
}
