import type { DownloadJobDto, EpisodeDto } from "@peregrine/ipc-client";
import { beforeEach, describe, expect, it, vi } from "vitest";

// The season request is the one write this module performs, so the command is stubbed and the
// module is imported after the stub is in place — the plan tests below never reach a transport.
const ipc = vi.hoisted(() => ({
  acquireSeason: vi.fn(async () => undefined),
}));

vi.mock("@peregrine/ipc-client", () => ({
  acquireSeason: ipc.acquireSeason,
}));

const { requestSeasonDownload, seasonDownloadPlan, seasonEpisodeKey } =
  await import("./season-download.js");

beforeEach(() => {
  ipc.acquireSeason.mockClear();
});

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
    isPreflight: false,
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

  it("reads a live pack row as the whole season, with its own percent and nothing missing", () => {
    // The reported defect: a pack row names no episode, so matching rows by episode number left the
    // season control offering the whole season again while the pack that was fetching it ran.
    const result = plan(
      [episode({ episodeNumber: 1 }), episode({ episodeNumber: 2 })],
      [job({ episodeNumber: null, state: "downloading", progressPermille: 450 })],
    );

    expect(result.status).toBe("active");
    expect(result.percent).toBe(45);
    expect(result.missing).toEqual([]);
    expect(result.pack).toEqual({ season: 1, status: "active", percent: 45, count: 1 });
  });

  it("reports a queued pack as the season queued rather than as missing episodes", () => {
    const result = plan([episode()], [job({ episodeNumber: null, state: "queued" })]);

    expect(result.status).toBe("queued");
    expect(result.missing).toEqual([]);
    expect(result.pack?.status).toBe("queued");
  });

  it("treats a paused pack as the active season", () => {
    const result = plan(
      [episode()],
      [job({ episodeNumber: null, state: "paused", progressPermille: 620 })],
    );

    expect(result.status).toBe("active");
    expect(result.percent).toBe(62);
    expect(result.pack?.status).toBe("active");
  });

  it("falls back to the episodes when the pack row failed or was removed", () => {
    // A failed pack is not a season under way; the control must offer the retry the per-episode plan
    // already knows how to describe. A removed row says nothing at all.
    for (const state of ["failed", "removed"]) {
      const result = plan([episode()], [job({ episodeNumber: null, state })]);

      expect(result.status).toBe("download");
      expect(result.missing).toHaveLength(1);
      expect(result.pack).toBeNull();
    }
  });

  it("ignores a pack row for another season or another title", () => {
    for (const row of [
      job({ episodeNumber: null, seasonNumber: 2, state: "downloading", progressPermille: 450 }),
      job({ episodeNumber: null, titleId: 2, state: "downloading", progressPermille: 450 }),
    ]) {
      const result = plan([episode()], [row]);

      expect(result.status).toBe("download");
      expect(result.pack).toBeNull();
      expect(result.missing).toHaveLength(1);
    }
  });
});

describe("requestSeasonDownload", () => {
  it("asks the core for the season once rather than for each missing episode", async () => {
    await requestSeasonDownload(7, 2);

    expect(ipc.acquireSeason).toHaveBeenCalledTimes(1);
    expect(ipc.acquireSeason).toHaveBeenCalledWith(7, 2);
  });
});
