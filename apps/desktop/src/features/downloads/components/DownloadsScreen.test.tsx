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

import { DownloadsScreen } from "./DownloadsScreen.js";

const i18n = createI18n();

function job(overrides: Partial<DownloadJobDto> = {}): DownloadJobDto {
  return {
    id: 1,
    titleId: 7,
    title: "The Matrix",
    titleKind: "movie",
    releaseTitle: "The.Matrix.1999.1080p",
    state: "downloading",
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
    ...overrides,
  };
}

function markup(rows: readonly DownloadJobDto[]): string {
  return renderToStaticMarkup(
    <I18nextProvider i18n={i18n}>
      <DownloadsScreen
        downloads={{ downloads: rows, loading: false, failed: false, refresh: () => Promise.resolve() }}
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
});
