import { createI18n } from "@peregrine/i18n";
import type { IpcError, RailItemDto } from "@peregrine/ipc-client";
import { renderToStaticMarkup } from "react-dom/server";
import { I18nextProvider } from "react-i18next";
import { describe, expect, it } from "vitest";

import type { TitleFeed } from "../hooks/useTitleFeed.js";
import { TitleFeedList } from "./TitleFeedList.js";

const i18n = createI18n();

function item(overrides: Partial<RailItemDto> = {}): RailItemDto {
  return {
    titleId: 7,
    kind: "movie",
    name: "The Matrix",
    year: 1999,
    poster: null,
    backdrop: null,
    playable: false,
    rating: null,
    overview: null,
    votes: null,
    ...overrides,
  };
}

function feed(overrides: Partial<TitleFeed> = {}): TitleFeed {
  return {
    items: [item()],
    loading: false,
    failed: null,
    exhausted: false,
    loadMore: () => undefined,
    ...overrides,
  };
}

function markup(state: TitleFeed): string {
  return renderToStaticMarkup(
    <I18nextProvider i18n={i18n}>
      <TitleFeedList feed={state} onNavigate={() => undefined} />
    </I18nextProvider>,
  );
}

describe("TitleFeedList", () => {
  it("announces a page that is on its way instead of leaving the list silent", () => {
    // The feed appends into the page's own scroller, so a reader waiting at the foot has nothing to
    // see while a page is in the air: the busy state and the status line are the whole signal.
    const html = markup(feed({ loading: true }));

    expect(html).toContain('aria-busy="true"');
    expect(html).toMatch(/role="status"[^>]*>Loading more titles…</u);
    expect(html).toContain("/title/movie/7");
  });

  it("reports a failed page before the feed is exhausted, keeping what arrived", () => {
    // A failure mid-feed leaves the sentinel in place for the next look, but staying silent made the
    // list look finished rather than short.
    const failed: IpcError = {
      code: "PROVIDER_UNAVAILABLE",
      message: "the provider could not answer",
      retryable: true,
    };
    const html = markup(feed({ failed }));

    expect(html).toContain("What arrived is above. Scroll again to try for the rest.");
    expect(html).toMatch(/role="status"[^>]*>What arrived is above/u);
    expect(html).toContain('aria-busy="false"');
    expect(html).not.toContain("Loading more titles…");
  });

  it("announces a feed that failed before it drew a single title", () => {
    const failed: IpcError = {
      code: "PROVIDER_UNAVAILABLE",
      message: "the provider could not answer",
      retryable: true,
    };
    const html = markup(feed({ items: [], failed }));

    expect(html).toMatch(/role="status"/u);
    expect(html).toContain("More titles could not be loaded");
    expect(html).toContain("What arrived is above. Scroll again to try for the rest.");
  });

  it("says the feed has ended rather than letting the list simply stop", () => {
    const html = markup(feed({ exhausted: true }));

    expect(html).toMatch(
      /role="status"[^>]*>That is everything your providers have for this title.</u,
    );
    expect(html).toContain('aria-busy="false"');
  });

  it("names the region and draws each title as a link", () => {
    const html = markup(feed({ items: [item({ titleId: 12, name: "The Matrix Reloaded" })] }));

    expect(html).toContain('aria-labelledby="title-feed-heading"');
    expect(html).toContain("More like this");
    expect(html).toContain("The Matrix Reloaded, 1999 · Film");
    expect(html).toContain('href="/title/movie/12"');
  });
});
