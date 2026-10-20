/**
 * The drawing, from synthetic DTOs.
 *
 * No network, no Tauri bridge, and no store: `BrowseView` takes what the hooks answered as props,
 * which is the seam that makes the toolbar, the grid, the search results, and the empty states
 * testable in a Node environment. What this file *cannot* do is scroll the grid, change a filter, or
 * type — `renderToStaticMarkup` is one pass with no DOM and no event loop — so the end-of-list rule
 * is `packages/ui`'s `gridEndReached` test, the page-merge rule is `paged-list.test.ts`, the search
 * hook's debounce is `lib/title-search.ts`, and the filter round trip through the core is the command
 * contract's.
 */

import { createI18n } from "@peregrine/i18n";
import type { GenreDto, IpcError, RailItemDto } from "@peregrine/ipc-client";
import { renderToStaticMarkup } from "react-dom/server";
import { I18nextProvider } from "react-i18next";
import { describe, expect, it } from "vitest";

import type { DownloadMarks, DownloadTitleSource } from "../../../lib/download-status.js";
import type { TileSearch } from "../../../lib/title-search.js";
import type { BrowseData } from "../hooks/useBrowse.js";
import { DEFAULT_FILTERS } from "../hooks/useBrowse.js";
import { BrowseView } from "./BrowseScreen.js";

const i18n = createI18n();

function item(overrides: Partial<RailItemDto> = {}): RailItemDto {
  return {
    titleId: 7,
    kind: "movie",
    name: "The Matrix",
    year: 1999,
    // `null` rather than a path: a Node test has no asset protocol, so a poster reference could not
    // become a URL and the markup below is the reserved box either way.
    poster: null,
    backdrop: null,
    playable: false,
    rating: null,
    overview: null,
    votes: null,
    ...overrides,
  };
}

function data(overrides: Partial<BrowseData> = {}): BrowseData {
  return {
    items: [item()],
    filters: DEFAULT_FILTERS,
    queryKey: "movie:null:popularity:null",
    hasMore: false,
    loading: false,
    failed: null,
    setFilters: () => undefined,
    loadMore: () => undefined,
    ...overrides,
  };
}

function tileSearch(overrides: Partial<TileSearch> = {}): TileSearch {
  return {
    query: "",
    setQuery: () => undefined,
    results: [],
    searching: false,
    failed: false,
    searched: false,
    searchNow: () => undefined,
    ...overrides,
  };
}

function markup(
  overrides: Partial<BrowseData> = {},
  genres: readonly GenreDto[] = [],
  kind: "movie" | "show" = "movie",
  downloads?: DownloadMarks,
  downloadRows?: readonly DownloadTitleSource[],
  searchOverrides: Partial<TileSearch> = {},
): string {
  return renderToStaticMarkup(
    <I18nextProvider i18n={i18n}>
      <BrowseView
        data={data(overrides)}
        downloadRows={downloadRows}
        downloads={downloads}
        genres={genres}
        kind={kind}
        onKind={() => undefined}
        onNavigate={() => undefined}
        search={tileSearch(searchOverrides)}
      />
    </I18nextProvider>,
  );
}

describe("BrowseView", () => {
  it("brings a title whose transfer is running to the front of the list", () => {
    // The provider's order survives inside each half; what moves is the transfer the reader
    // came back for.
    const html = markup(
      { items: [item({ titleId: 1, name: "First" }), item({ titleId: 2, name: "Second" })] },
      [],
      "movie",
      new Map([[2, "active" as const]]),
    );
    expect(html.indexOf("/title/movie/2")).toBeLessThan(html.indexOf("/title/movie/1"));
    // The tile that moved also wears the mark: ordering and the border are one fact.
    expect(html).toContain("bg-accent");
  });

  it("draws one tile per title, linked to the title's own screen", () => {
    const html = markup({
      items: [item({ titleId: 1, name: "The Matrix" }), item({ titleId: 2, name: "Inception" })],
    });

    expect(html).toContain("/title/movie/1");
    expect(html).toContain("/title/movie/2");
    // The accessible name a screen reader reads: the tile carries no visible caption.
    expect(html).toContain("The Matrix, 1999 · Film");
  });

  it("names a series as a series rather than as a film", () => {
    const html = markup({ items: [item({ kind: "show", name: "Severance", year: 2022 })] });

    expect(html).toContain("Severance, 2022 · Series");
  });

  it("offers a search field over the screen", () => {
    const html = markup();

    expect(html).toContain('role="search"');
    expect(html).toContain('placeholder="Search titles"');
    expect(html).toContain("Search for a title");
  });

  it("replaces the grid with a live search's poster tiles", () => {
    // The provider answered, and a title the loaded page never carried is the result. It is drawn by
    // the same card as a browse tile — a poster, a score, the year, and the hover panel.
    const html = markup({}, [], "movie", undefined, undefined, {
      query: "spiderman",
      results: [item({ titleId: 9, name: "Spider-Man: Brand New Day", year: 2026, rating: 8.5 })],
      searched: true,
    });

    expect(html).toContain("Spider-Man: Brand New Day, 2026 · Film");
    expect(html).toContain("/title/movie/9");
    expect(html).toContain("Search results");
    expect(html).not.toContain("Browse results");
    // The grid under the field changes without focus moving, so the live region says how many
    // titles arrived.
    expect(html).toMatch(/role="status"[^>]*>1 result</u);
    // The provider filters belong to the grid; a live search disables them rather than letting a
    // change spend a request on a page nobody can see.
    expect(html).toContain('disabled=""');
  });

  it("keeps the grid for a query too short to search", () => {
    const html = markup({}, [], "movie", undefined, undefined, { query: "s" });

    expect(html).toContain("Browse results");
    expect(html).toContain("/title/movie/7");
  });

  it("says a search is running, and says when one matched nothing", () => {
    const running = markup({}, [], "movie", undefined, undefined, {
      query: "spider",
      searching: true,
    });
    expect(running).toMatch(/role="status"[^>]*>Searching the providers…</u);

    const nothing = markup({}, [], "movie", undefined, undefined, {
      query: "zzz",
      searched: true,
    });
    expect(nothing).toContain("Nothing matched");
    expect(nothing).toMatch(/role="status"[^>]*>Nothing matched</u);
  });

  it("reports a failed search rather than calling it empty", () => {
    const html = markup({}, [], "movie", undefined, undefined, {
      query: "spider",
      failed: true,
      searched: true,
    });

    expect(html).toContain("The search could not be made");
    expect(html).not.toContain("Nothing matched");
  });

  it("offers the provider's genres as a filter, with the unfiltered choice first", () => {
    const genres: readonly GenreDto[] = [
      { id: 28, name: "Action" },
      { id: 35, name: "Comedy" },
    ];
    const html = markup({}, genres);

    // The provider's own identifier is the option's value, because that is what a browse request
    // hands back to it; the name is the label.
    expect(html).toContain('<option value="28">Action</option>');
    expect(html).toContain('<option value="35">Comedy</option>');
    expect(html).toContain("All genres");
  });

  it("offers every order and a score floor", () => {
    const html = markup();

    expect(html).toContain("Most popular");
    expect(html).toContain("Best rated");
    expect(html).toContain("Newest");
    expect(html).toContain("Oldest");
    expect(html).toContain("Any rating");
    expect(html).toContain("8+ stars");
    // The three filters are raw selects rather than kit controls, so their visible focus state is
    // this screen's to own alongside the search pill's focus-within ring.
    const selects = [...html.matchAll(/<select[^>]*>/gu)].map((match) => match[0]);
    expect(selects).toHaveLength(3);
    for (const select of selects) {
      expect(select).toContain("focus-visible:outline-2");
    }
  });

  it("marks the kind the route named as the one pressed", () => {
    // `aria-pressed` is what a screen reader reads as the current category, and the two controls are
    // the only way to change it.
    const films = markup({}, [], "movie");
    const shows = markup({}, [], "show");

    expect(films).toContain('aria-pressed="true">Movies<');
    expect(films).toContain('aria-pressed="false">TV shows<');
    expect(shows).toContain('aria-pressed="true">TV shows<');
  });

  it("says nothing rather than something wrong while the first page is in the air", () => {
    // `DESIGN_SYSTEM §11` rule 2: no spinner and no empty state that is really a wait. The screen
    // names itself — which is PG-E7-02's acceptance — and draws no claim about the catalogue.
    const html = markup({ items: [], loading: true });

    expect(html).toContain("<h1");
    expect(html).not.toContain("Nothing matches these filters");
    expect(html).not.toContain('role="list"');
  });

  it("draws an empty state when the filters matched nothing", () => {
    const html = markup({ items: [], loading: false });

    expect(html).toContain("Nothing matches these filters");
    expect(html).not.toContain('role="list"');
  });

  it("reports a failed page and keeps what it already has", () => {
    // A provider having a bad minute is a reason to say so, not to blank a grid the reader was
    // looking at: the titles already received stay on screen, and the failure is a line above them.
    const failed: IpcError = {
      code: "PROVIDER_UNAVAILABLE",
      message: "the provider could not answer",
      retryable: true,
    };
    const html = markup({ items: [item({ name: "The Matrix" })], failed });

    expect(html).toContain("This page could not be loaded");
    expect(html).toContain("/title/movie/7");
  });

  it("says a later page is loading without taking the grid away", () => {
    const html = markup({ items: [item()], loading: true, hasMore: true });

    expect(html).toContain("Loading more…");
    expect(html).toContain("/title/movie/7");
  });

  it("reserves a placeholder tile at the grid's own geometry while a page is in the air", () => {
    // The affordance the reader actually sees: a skeleton where the arriving tiles will be, at the
    // `lg` tile's 176 × 264. A line of text under a wall of posters is easy to miss; the shape is
    // what says "more of these".
    const html = markup({ items: [item()], loading: true, hasMore: true });

    expect(html).toContain("height:264px");
    expect(html).toContain("width:176px");
  });

  it("says the list has ended rather than leaving the reader to guess", () => {
    // The other half of the same question. Nothing to load and nothing more to come are different
    // answers, and a reader who cannot tell them apart keeps scrolling a page that will not grow.
    const html = markup({ items: [item()], hasMore: false, loading: false });

    expect(html).toContain("That is everything for these filters.");
  });

  it("does not claim the end while a page is still on its way", () => {
    const html = markup({ items: [item()], hasMore: true, loading: true });

    expect(html).not.toContain("That is everything for these filters.");
  });

  it("puts a download the provider did not send above the loaded page", () => {
    // The whole point of the global merge: a niche title the reader downloaded is not on the
    // provider's page, and it still opens Browse at the top rather than being invisible.
    const html = markup(
      { items: [item({ titleId: 1, name: "Provider" })] },
      [],
      "movie",
      new Map([[9, "done" as const]]),
      [
        {
          titleId: 9,
          title: "Downloaded",
          titleKind: "movie",
          poster: null,
          state: "completed",
          hasMediaFile: false,
        },
      ],
    );
    expect(html).toContain("Downloaded");
    expect(html).toContain("/title/movie/9");
    expect(html.indexOf("/title/movie/9")).toBeLessThan(html.indexOf("/title/movie/1"));
  });
});
