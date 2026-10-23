/**
 * The drawing, from synthetic DTOs.
 *
 * No bridge and no store: the screen is a projection of one read, so a row drawn from a hand-made
 * DTO is enough to hold the one move this file owns — a title that resolves becomes a link to its
 * own route, and one that does not stays text.
 */

import { createI18n } from "@peregrine/i18n";
import type { DownloadJobDto } from "@peregrine/ipc-client";
import { renderToStaticMarkup } from "react-dom/server";
import { I18nextProvider } from "react-i18next";
import { describe, expect, it } from "vitest";

import { downloadTree } from "./download-tree.js";
import { DownloadsScreen } from "./DownloadsScreen.js";

const i18n = createI18n();

function job(overrides: Partial<DownloadJobDto> = {}): DownloadJobDto {
  return {
    id: 1,
    titleId: 7,
    title: "The Matrix",
    titleKind: "movie",
    poster: null,
    seasonNumber: null,
    episodeNumber: null,
    releaseTitle: "The.Matrix.1999.1080p",
    state: "downloading",
    protocol: "torrent",
    stage: "transfer",
    progressPermille: 500,
    reportedSizeBytes: 1_000,
    downloadedBytes: 500,
    downloadRateBps: 1,
    peersConnected: 1,
    peersSwarm: 2,
    etaSeconds: 10,
    addedAt: 0,
    errorCode: null,
    errorMessage: null,
    findings: [],
    hasMediaFile: true,
    ...overrides,
  };
}

function seasonJob(overrides: Partial<DownloadJobDto> = {}): DownloadJobDto {
  return job({
    id: 2,
    title: "Severance",
    titleKind: "show",
    seasonNumber: 1,
    episodeNumber: 1,
    ...overrides,
  });
}

function markup(rows: readonly DownloadJobDto[]): string {
  return renderToStaticMarkup(
    <I18nextProvider i18n={i18n}>
      <DownloadsScreen
        downloads={{
          downloads: rows,
          loading: false,
          failed: false,
          refresh: () => Promise.resolve(),
        }}
        onNavigate={() => undefined}
      />
    </I18nextProvider>,
  );
}

describe("DownloadsScreen", () => {
  it("opens the title a row is filling", () => {
    expect(markup([job()])).toContain('href="/title/movie/7"');
  });

  it("keeps a row whose title cannot be resolved as text rather than a guessed link", () => {
    const html = markup([job({ title: null, titleKind: null })]);
    expect(html).toContain("The.Matrix.1999.1080p");
    expect(html).not.toContain('href="/title/');
  });

  it("says 99 percent for a payload one piece short of whole", () => {
    const html = markup([job({ progressPermille: 999 })]);
    expect(html).toContain("99%");
    expect(html).not.toContain("100%");
  });

  it("offers a peer search on a live torrent and not on a finished one", () => {
    expect(markup([job()])).toContain("Find peers");
    expect(markup([job({ state: "completed", progressPermille: 1000 })])).not.toContain(
      "Find peers",
    );
  });

  it("shows the reason a failed row stopped", () => {
    const html = markup([
      job({ state: "failed", errorMessage: "no peers answered after 3 attempts" }),
    ]);
    expect(html).toContain("no peers answered after 3 attempts");
  });

  it("groups one season's episodes under one header with the floored aggregate", () => {
    const html = markup([
      seasonJob({ id: 11, episodeNumber: 1, progressPermille: 500 }),
      seasonJob({ id: 12, episodeNumber: 2, progressPermille: 900 }),
    ]);

    // One disclosure for the season, not one per episode.
    expect(html.match(/aria-expanded="true"/gu)).toHaveLength(1);
    expect(html).toContain("Season 1");
    expect(html).toContain("2 episodes");
    // (50 + 90) / 2, floored.
    expect(html).toContain("70%");

    const header = html.indexOf('aria-expanded="true"');
    expect(header).toBeGreaterThan(-1);
    expect(html.indexOf("S1E01")).toBeGreaterThan(header);
    expect(html.indexOf("S1E02")).toBeGreaterThan(header);
    // Both episode rows are indented under the header rather than sitting at its level.
    expect(html.match(/ps-6/gu)).toHaveLength(2);
  });

  it("keeps a row with no season flat instead of inventing a group", () => {
    const html = markup([job()]);
    expect(html).toContain("The Matrix");
    expect(html).not.toContain("aria-expanded");
    expect(html).not.toContain("episodes");
  });

  it("does not round a season to 100 while its only episode is at 99 percent", () => {
    const html = markup([seasonJob({ id: 13, progressPermille: 999 })]);
    expect(html).toContain("99%");
    expect(html).not.toContain("100%");
  });

  it("draws the disclosure expanded by default with its own focus ring", () => {
    const html = markup([seasonJob({ id: 14 })]);
    expect(html).toContain('aria-label="Collapse season 1 of Severance"');
    expect(html).toMatch(/<button[^>]*aria-expanded="true"[^>]*focus-visible:outline-2/u);
    expect(html).toContain('type="button"');
  });

  it("counts a season pack as the season's own row, not as an episode", () => {
    const html = markup([
      seasonJob({ id: 21, episodeNumber: null, downloadedBytes: 40, hasMediaFile: false }),
      seasonJob({ id: 22, episodeNumber: 1, progressPermille: 500 }),
      seasonJob({ id: 23, episodeNumber: 2, progressPermille: 700 }),
    ]);
    expect(html).toContain("2 episodes");
    // Every row under the header keeps its own actions, the pack included.
    expect(html.match(/More actions/gu)).toHaveLength(3);
  });

  it("says one pack for a lone live pack instead of counting zero episodes", () => {
    const html = markup([seasonJob({ id: 24, episodeNumber: null, progressPermille: 450 })]);
    expect(html).toContain("1 pack");
    // The season bar reads the pack's own percent, not only the row's copy of it.
    expect(html).toMatch(
      /aria-label="Severance season 1 download progress"[^>]*aria-valuenow="45"/u,
    );
    expect(html).not.toContain("0 episodes");
  });

  it("shows a pack beside the episodes it accompanies, with the episodes' floored aggregate", () => {
    const html = markup([
      seasonJob({ id: 25, episodeNumber: null, progressPermille: 200 }),
      seasonJob({ id: 26, episodeNumber: 1, progressPermille: 500 }),
      seasonJob({ id: 27, episodeNumber: 2, progressPermille: 900 }),
    ]);
    expect(html).toContain("2 episodes + 1 pack");
    // (50 + 90) / 2, floored: the pack's own 20% stays on its row rather than the header.
    expect(html).toContain("70%");
  });

  it("falls back to the season's episodes for a failed pack", () => {
    const html = markup([
      seasonJob({ id: 28, episodeNumber: null, state: "failed", progressPermille: 400 }),
    ]);
    expect(html).toContain("0 episodes");
    expect(html).not.toContain("1 pack");
  });

  it("counts one episode when a retry sits beside its replacement", () => {
    const html = markup([
      seasonJob({ id: 31, episodeNumber: 1, state: "completed", progressPermille: 1000 }),
      seasonJob({ id: 32, episodeNumber: 1, state: "failed", progressPermille: 400 }),
      seasonJob({ id: 33, episodeNumber: 2, progressPermille: 500 }),
    ]);
    expect(html).toContain("2 episodes");
    // The first row for an episode is the one the season reads, as seasonDownloadPlan does.
    expect(html).toContain("75%");
    expect(html.match(/More actions/gu)).toHaveLength(3);
  });
});

describe("downloadTree", () => {
  it("collapses one season without hiding another", () => {
    const rows = [
      seasonJob({ id: 41, seasonNumber: 1, episodeNumber: 1 }),
      seasonJob({ id: 42, seasonNumber: 2, episodeNumber: 1 }),
    ];
    const [first] = downloadTree(rows, new Set());
    if (first?.kind !== "season") {
      throw new Error("the first line should be a season header");
    }

    const lines = downloadTree(rows, new Set([first.key]));
    expect(lines).toHaveLength(3);
    expect(lines.filter((line) => line.kind === "season")).toHaveLength(2);
    expect(lines.filter((line) => line.kind === "job")).toHaveLength(1);
  });

  it("keeps a row without a season where the queue put it", () => {
    const lines = downloadTree([job({ id: 51 }), seasonJob({ id: 52 })], new Set());
    expect(lines.map((line) => line.kind)).toEqual(["job", "season", "job"]);
    expect(lines[0]).toMatchObject({ kind: "job", nested: false });
    expect(lines[2]).toMatchObject({ kind: "job", nested: true });
  });

  it("keeps the same season number of two titles apart", () => {
    const rows = [seasonJob({ id: 61, titleId: 7 }), seasonJob({ id: 62, titleId: 8 })];
    const seasons = downloadTree(rows, new Set()).filter((line) => line.kind === "season");
    expect(seasons).toHaveLength(2);
  });

  it("keeps a pack for one season in its own group", () => {
    const rows = [
      seasonJob({ id: 71, seasonNumber: 1, episodeNumber: null }),
      seasonJob({ id: 72, seasonNumber: 2, episodeNumber: 1 }),
    ];
    const seasons = downloadTree(rows, new Set()).filter((line) => line.kind === "season");
    expect(seasons).toHaveLength(2);
    expect(seasons[0]).toMatchObject({ seasonNumber: 1, episodeCount: 0, packCount: 1 });
    expect(seasons[1]).toMatchObject({ seasonNumber: 2, episodeCount: 1, packCount: 0 });
  });

  it("counts a queued or paused pack as live and a failed or removed one as not", () => {
    for (const state of ["queued", "paused", "failed", "removed"] as const) {
      const [line] = downloadTree([seasonJob({ id: 73, episodeNumber: null, state })], new Set());
      expect(line).toMatchObject({
        kind: "season",
        packCount: state === "failed" || state === "removed" ? 0 : 1,
      });
    }
  });
});
