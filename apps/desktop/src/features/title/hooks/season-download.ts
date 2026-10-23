/**
 * What a season's download control is doing, from the season's episodes and the queue rows.
 *
 * The plan is a projection: it names which episodes are still missing and how far the whole season
 * has got. The page asks the core for the season once — the core chooses the pack or the episodes
 * and, when the episodes win, fans out inside that one job — so the queue rows, not this module,
 * remain the authority once a job exists.
 */

import { acquireSeason } from "@peregrine/ipc-client";
import type { DownloadJobDto, EpisodeDto } from "@peregrine/ipc-client";

import { downloadPercentOf, downloadStatusOf } from "../../../lib/download-status.js";
import { episodeAvailability } from "./seasons.js";

/** What the season control should say. */
export type SeasonDownloadStatus = "download" | "queued" | "active" | "done" | "retry";

/**
 * A live pack row covering a whole season.
 *
 * A pack names no episode, so it is not a member of the episodes this plan otherwise walks. It is
 * the season's own transfer: its progress is the season's progress, and while it is live there is
 * nothing left for a second request to acquire.
 */
export interface SeasonPack {
  /** The plan's season, so a card can say which pack is covering it. */
  readonly season: number;
  /** A queued pack has not started; anything the engine is moving is active. */
  readonly status: "queued" | "active";
  /** The pack's own completion, 0-100. */
  readonly percent: number;
  /** How many live pack rows cover the season, so the control can say "1 pack". */
  readonly count: number;
}

/** One season's control state. */
export interface SeasonDownloadPlan {
  readonly status: SeasonDownloadStatus;
  /** Whole-season completion, 0-100. */
  readonly percent: number;
  /** Episodes a click should acquire, in season order. */
  readonly missing: readonly EpisodeDto[];
  /** The live pack covering the whole season, or null when the per-episode plan is the one to use. */
  readonly pack: SeasonPack | null;
}

/** What the plan reads. */
export interface SeasonDownloadInput {
  readonly titleId: number;
  readonly season: number;
  readonly episodes: readonly EpisodeDto[];
  readonly downloads: readonly DownloadJobDto[];
  /** Episode keys the page has asked for but the queue rows have not caught up with. */
  readonly requested: ReadonlySet<string>;
  /** The page is currently issuing this season's requests. */
  readonly busy: boolean;
}

/** The key the page uses for one episode's request. */
export function seasonEpisodeKey(season: number, episode: number): string {
  return `${String(season)}:${String(episode)}`;
}

/**
 * Asks the core to acquire one whole season in a single job.
 *
 * The missing plan is the screen's, but what to fetch is the core's: a season's episodes may beat
 * its pack on peers, and the job fans out over the missing episodes itself rather than making the
 * page issue one command per episode.
 */
export async function requestSeasonDownload(titleId: number, season: number): Promise<void> {
  await acquireSeason(titleId, season);
}

/**
 * The live pack row covering a season, if one exists.
 *
 * Only a live row covers the season. A failed, blocked, or removed pack is the per-episode plan's
 * to retry rather than a season the page should keep claiming is under way, and a row for another
 * title or another season says nothing about this one. When a retry sits beside its predecessor,
 * the active row wins over the queued one and the furthest along wins within that.
 */
function livePack(
  downloads: readonly DownloadJobDto[],
  titleId: number,
  season: number,
): SeasonPack | null {
  let chosen: DownloadJobDto | null = null;
  let status: "queued" | "active" = "queued";
  let count = 0;
  for (const row of downloads) {
    if (
      row.titleId !== titleId ||
      row.seasonNumber !== season ||
      row.episodeNumber !== null ||
      downloadStatusOf(row.state) !== "active"
    ) {
      continue;
    }
    count += 1;
    const candidate = row.state === "queued" ? "queued" : "active";
    if (
      chosen === null ||
      (status === "queued" && candidate === "active") ||
      (status === candidate && row.progressPermille > chosen.progressPermille)
    ) {
      chosen = row;
      status = candidate;
    }
  }
  return chosen === null
    ? null
    : { season, status, percent: downloadPercentOf(chosen.progressPermille), count };
}

/** One season's control state. */
export function seasonDownloadPlan({
  titleId,
  season,
  episodes,
  downloads,
  requested,
  busy,
}: SeasonDownloadInput): SeasonDownloadPlan {
  // **A pack is the season's own job, and it names no episode.** Matching the queue's rows by
  // episode number alone would leave the control offering the whole season again while the pack
  // that is already fetching it runs, which is the stale page this plan exists to prevent.
  const pack = livePack(downloads, titleId, season);
  if (pack !== null) {
    return { status: pack.status, percent: pack.percent, missing: [], pack };
  }
  const held = episodes.filter((episode) => episode.seasonNumber === season);
  if (held.length === 0) {
    return { status: "download", percent: 0, missing: [], pack: null };
  }
  let active = 0;
  let queued = 0;
  let failed = 0;
  let done = 0;
  let percent = 0;
  const missing: EpisodeDto[] = [];
  for (const episode of held) {
    const key = seasonEpisodeKey(episode.seasonNumber, episode.episodeNumber);
    const job = downloads.find(
      (row) =>
        row.titleId === titleId &&
        row.seasonNumber === episode.seasonNumber &&
        row.episodeNumber === episode.episodeNumber,
    );
    const status = job === undefined ? null : downloadStatusOf(job.state);
    const availability = episodeAvailability(episode);
    if (availability === "available" || availability === "partial" || status === "done") {
      done += 1;
      percent += 100;
    } else if (status === "active" && job?.state !== "queued") {
      active += 1;
      percent += downloadPercentOf(job?.progressPermille ?? 0);
    } else if (job?.state === "queued" || (job === undefined && requested.has(key))) {
      queued += 1;
      missing.push(episode);
    } else {
      if (status === "failed") failed += 1;
      missing.push(episode);
    }
  }
  // Floored for the same reason a single transfer is: the control says 100 only when every
  // episode is done, and an average that rounded up would claim a finished season with one
  // episode still missing bytes.
  const whole = Math.floor(percent / held.length);
  if (active > 0) return { status: "active", percent: whole, missing, pack: null };
  if (queued > 0) return { status: "queued", percent: whole, missing, pack: null };
  if (done === held.length) return { status: "done", percent: 100, missing: [], pack: null };
  if (busy) return { status: "queued", percent: whole, missing, pack: null };
  if (failed > 0) return { status: "retry", percent: whole, missing, pack: null };
  return { status: "download", percent: whole, missing, pack: null };
}
