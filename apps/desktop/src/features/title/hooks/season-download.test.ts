import type { DownloadJobDto, EpisodeDto } from "@peregrine/ipc-client";
import { describe, expect, it } from "vitest";

import { seasonDownloadPlan, seasonEpisodeKey } from "./season-download.js";

function episode(overrides: Partial<EpisodeDto> = {}): EpisodeDto {
  return {
    seasonNumber: 1,
    episodeNumber: 1,
    name: null,
    overview: null,
    airDate: null,
    runtimeMinutes: null,
    rating: null,
    still: null,
    items: 0,
    files: 0,
    bytes: 0,
    missingFiles: 0,
    watched: false,
    progress: null,
    ...overrides,
  };
}

function job(overrides: Partial<DownloadJobDto> = {}): DownloadJobDto {
  return {
    id: 1,
    titleId: 1,
    title: null,
    titleKind: null,
    poster: null,
    seasonNumber: 1,
    episodeNumber: 1,
    releaseTitle: "Fixture",
    state: "downloading",
    protocol: "torrent",
    stage: "downloading",
    progressPermille: 0,
    reportedSizeBytes: null,
    downloadedBytes: 0,
    downloadRateBps: 0,
    peersConnected: 0,
    peersSwarm: 0,
    etaSeconds: null,
    addedAt: 0,
    errorCode: null,
    errorMessage: null,
    findings: [],
    hasMediaFile: false,
    ...overrides,
  };
}

function plan(
  episodes: readonly EpisodeDto[],
  downloads: readonly DownloadJobDto[] = [],
  requested: ReadonlySet<string> = new Set(),
  busy = false,
) {
  return seasonDownloadPlan({
    titleId: 1,
    season: 1,
    episodes,
    downloads,
    requested,
    busy,
  });
}

describe("seasonDownloadPlan", () => {
  it("offers a download for the missing episodes", () => {
    const result = plan([episode(), episode({ episodeNumber: 2 })]);
    expect(result.status).toBe("download");
    expect(result.missing).toHaveLength(2);
  });

  it("skips episodes that are already on disk", () => {
    const result = plan([
      episode({ episodeNumber: 1, items: 1, files: 1, missingFiles: 0 }),
      episode({ episodeNumber: 2 }),
    ]);
    expect(result.status).toBe("download");
    expect(result.percent).toBe(50);
    expect(result.missing.map((held) => held.episodeNumber)).toEqual([2]);
  });

  it("reports a queued season, including a click the rows have not caught up with", () => {
    expect(plan([episode()], [job({ state: "queued" })]).status).toBe("queued");
    expect(plan([episode()], [], new Set([seasonEpisodeKey(1, 1)])).status).toBe("queued");
  });

  it("reports the whole season percentage while a transfer is active", () => {
    const result = plan(
      [
        episode({ episodeNumber: 1, items: 1, files: 1, missingFiles: 0 }),
        episode({ episodeNumber: 2 }),
        episode({ episodeNumber: 3 }),
      ],
      [job({ episodeNumber: 2, state: "downloading", progressPermille: 400 })],
    );
    expect(result.status).toBe("active");
    expect(result.percent).toBe(46);
  });

  it("does not round a season to 100 while an episode is short", () => {
    const result = plan(
      [
        episode({ episodeNumber: 1, items: 1, files: 1, missingFiles: 0 }),
        episode({ episodeNumber: 2 }),
      ],
      [job({ episodeNumber: 2, state: "downloading", progressPermille: 999 })],
    );
    expect(result.status).toBe("active");
    expect(result.percent).toBe(99);
  });

  it("says done only when every episode is on disk", () => {
    const result = plan([
      episode({ episodeNumber: 1, items: 1, files: 1, missingFiles: 0 }),
      episode({ episodeNumber: 2, items: 1, files: 1, missingFiles: 0 }),
    ]);
    expect(result.status).toBe("done");
    expect(result.missing).toEqual([]);
  });

  it("offers a retry after a failure and while the page is issuing requests", () => {
    expect(plan([episode()], [job({ state: "failed" })]).status).toBe("retry");
    expect(plan([episode()], [], new Set(), true).status).toBe("queued");
  });
});
