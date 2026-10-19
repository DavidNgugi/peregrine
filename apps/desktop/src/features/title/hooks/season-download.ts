/**
 * What a season's download control is doing, from the season's episodes and the queue rows.
 *
 * The desktop has per-episode acquisitions rather than a server-side season command, so the plan
 * is a projection: it names which episodes are still missing and how far the whole season has got.
 * The page starts one acquisition per missing episode; the queue rows, not this module, remain the
 * authority once a job exists.
 */

import type { DownloadJobDto, EpisodeDto } from "@peregrine/ipc-client";

import { downloadStatusOf } from "../../../lib/download-status.js";
import { episodeAvailability } from "./seasons.js";

/** What the season control should say. */
export type SeasonDownloadStatus = "download" | "queued" | "active" | "done" | "retry";

/** One season's control state. */
export interface SeasonDownloadPlan {
  readonly status: SeasonDownloadStatus;
  /** Whole-season completion, 0-100. */
  readonly percent: number;
  /** Episodes a click should acquire, in season order. */
  readonly missing: readonly EpisodeDto[];
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

/** One season's control state. */
export function seasonDownloadPlan({
  titleId,
  season,
  episodes,
  downloads,
  requested,
  busy,
}: SeasonDownloadInput): SeasonDownloadPlan {
  const held = episodes.filter((episode) => episode.seasonNumber === season);
  if (held.length === 0) {
    return { status: "download", percent: 0, missing: [] };
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
      percent += Math.max(0, Math.min(100, Math.round((job?.progressPermille ?? 0) / 10)));
    } else if (job?.state === "queued" || (job === undefined && requested.has(key))) {
      queued += 1;
      missing.push(episode);
    } else {
      if (status === "failed") failed += 1;
      missing.push(episode);
    }
  }
  const whole = Math.round(percent / held.length);
  if (active > 0) return { status: "active", percent: whole, missing };
  if (queued > 0) return { status: "queued", percent: whole, missing };
  if (done === held.length) return { status: "done", percent: 100, missing: [] };
  if (busy) return { status: "queued", percent: whole, missing };
  if (failed > 0) return { status: "retry", percent: whole, missing };
  return { status: "download", percent: whole, missing };
}
