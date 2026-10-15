/**
 * The drawing, from synthetic DTOs.
 *
 * No network, no Tauri bridge, and no store: `BrowseView` takes what the hooks answered as props,
 * which is the seam that makes the toolbar, the grid, and both empty states testable in a Node
 * environment. What this file *cannot* do is scroll the grid or change a filter — `renderToStaticMarkup`
 * is one pass with no DOM and no event loop — so the end-of-list rule is `packages/ui`'s
 * `gridEndReached` test, the page-merge rule is `browse-pages.test.ts`, and the filter round trip
 * through the core is the command contract's.
 */

import { createI18n } from "@peregrine/i18n";
import type { GenreDto, IpcError, RailItemDto } from "@peregrine/ipc-client";
import { renderToStaticMarkup } from "react-dom/server";
import { I18nextProvider } from "react-i18next";
import { describe, expect, it } from "vitest";

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

function markup(
  overrides: Partial<BrowseData> = {},
  genres: readonly GenreDto[] = [],
  kind: "movie" | "show" = "movie",
): string {
  return renderToStaticMarkup(
    <I18nextProvider i18n={i18n}>
      <BrowseView
        data={data(overrides)}
        genres={genres}
        kind={kind}
        onKind={() => undefined}
        onNavigate={() => undefined}
      />
    </I18nextProvider>,
  );
}

describe("BrowseView", () => {
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
});
