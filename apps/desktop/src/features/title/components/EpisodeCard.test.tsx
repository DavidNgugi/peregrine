import { createI18n } from "@peregrine/i18n";
import type { EpisodeDto } from "@peregrine/ipc-client";
import { renderToStaticMarkup } from "react-dom/server";
import { I18nextProvider } from "react-i18next";
import { describe, expect, it } from "vitest";

import { EpisodeCard } from "./EpisodeCard.js";

const i18n = createI18n();

function episode(overrides: Partial<EpisodeDto> = {}): EpisodeDto {
  return {
    seasonNumber: 1,
    episodeNumber: 2,
    name: "Cat's in the Bag...",
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

function markup(downloading: boolean, streamable: boolean): string {
  return renderToStaticMarkup(
    <I18nextProvider i18n={i18n}>
      <EpisodeCard
        downloading={downloading}
        downloadPercent={12}
        episode={episode()}
        onDownload={() => undefined}
        onPlay={() => undefined}
        streamable={streamable}
      />
    </I18nextProvider>,
  );
}

describe("EpisodeCard", () => {
  it("offers Play while a transfer has named the episode's file", () => {
    const html = markup(true, true);

    expect(html).toContain("Play episode 1×02");
    expect(html).toContain("Downloading… 12%");
  });

  it("offers only Download before metadata names a file", () => {
    const html = markup(true, false);

    expect(html).not.toContain("Play episode 1×02");
    expect(html).toContain("Downloading… 12%");
  });

  it("says a live season pack covers the episode and offers no second acquisition", () => {
    const html = renderToStaticMarkup(
      <I18nextProvider i18n={i18n}>
        <EpisodeCard
          episode={episode()}
          onDownload={() => undefined}
          seasonPack={{ season: 1, status: "active", percent: 45, count: 1 }}
        />
      </I18nextProvider>,
    );

    expect(html).toContain("Downloading with season 1");
    expect(html).not.toContain("Not acquired");
    expect(html).toMatch(/<button[^>]*disabled=""[^>]*>Downloading… 45%<\/button>/u);
  });

  it("offers Schedule and no watch mark before an episode airs", () => {
    const html = renderToStaticMarkup(
      <I18nextProvider i18n={i18n}>
        <EpisodeCard
          episode={episode({ airDate: "2999-01-01" })}
          onSchedule={() => undefined}
          onToggleWatched={() => undefined}
          scheduled={false}
        />
      </I18nextProvider>,
    );

    expect(html).toContain("Schedule");
    expect(html).not.toContain("Mark watched");
  });
});
