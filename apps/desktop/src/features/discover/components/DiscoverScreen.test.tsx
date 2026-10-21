import { createI18n } from "@peregrine/i18n";
import type {
  ContinueWatchingDto,
  HomeRailDto,
  IpcError,
  LibraryTitleDto,
  RailItemDto,
  TitleMatchDto,
  WatchlistEntryDto,
} from "@peregrine/ipc-client";
import { renderToStaticMarkup } from "react-dom/server";
import { I18nextProvider } from "react-i18next";
import { describe, expect, it } from "vitest";
import type { HomeRailsView } from "../hooks/useHomeRails.js";
import { DiscoverView, itemMeta, type DiscoverViewProps } from "./DiscoverScreen.js";
import { heroSlides } from "./hero-rotation.js";

const i18n = createI18n();

/**
 * The drawing, from synthetic DTOs.
 *
 * No network, no Tauri bridge, and no store: `DiscoverView` takes what the hooks answered as props,
 * which is the seam that makes the rails, the hero, and every empty state testable in a Node
 * environment. What this file *cannot* do is drive the refresh click or the hero's interval —
 * `renderToStaticMarkup` is one pass with no DOM and no event loop, so the click and the
 * `job://state` follow are unobserved here; `rails-follow.test.ts` covers the matching rule, the
 * rotation's timing is `hero-rotation.test.ts`'s, and the shell exercises the hook itself.
 */
function item(overrides: Partial<RailItemDto> = {}): RailItemDto {
  return {
    titleId: 7,
    kind: "movie",
    name: "The Matrix",
    year: 1999,
    // `null` rather than a path: a Node test has no asset protocol, so a poster or backdrop
    // reference could not become a URL and the markup below is the reserved box either way.
    poster: null,
    backdrop: null,
    // `false` rather than true: most of what the rails hold is not something the library has, and a
    // test that wanted a playable title says so.
    playable: false,
    // `null` rather than a score: most of what the screen draws has no badge, and a test that wanted
    // one says so.
    rating: null,
    // `null` rather than prose or a count: the preview panel draws what the core answered and
    // nothing for a field the provider never sent, and a test that wanted either says so.
    overview: null,
    votes: null,
    ...overrides,
  };
}

function rail(overrides: Partial<HomeRailDto> = {}): HomeRailDto {
  return { rail: "popular_movies", items: [item()], state: "ready", ...overrides };
}

function railsView(overrides: Partial<HomeRailsView> = {}): HomeRailsView {
  return {
    rails: [rail()],
    state: "ready",
    loading: false,
    failed: false,
    refreshing: false,
    refreshError: null,
    refresh: () => Promise.resolve(),
    ...overrides,
  };
}

const NO_SEARCH = {
  results: [],
  searching: false,
  failed: false,
  searched: false,
  query: "",
  setQuery: () => undefined,
  source: "none",
  searchNow: () => undefined,
} as const;

function markup(view: HomeRailsView): string {
  return renderToStaticMarkup(
    <I18nextProvider i18n={i18n}>
      <DiscoverView onNavigate={() => undefined} rails={view} search={{ ...NO_SEARCH }} />
    </I18nextProvider>,
  );
}

/** The same, with the search panel in whatever state a test is about. */
function searched(overrides: Partial<DiscoverViewProps["search"]>): string {
  return renderToStaticMarkup(
    <I18nextProvider i18n={i18n}>
      <DiscoverView
        onNavigate={() => undefined}
        rails={railsView()}
        search={{ ...NO_SEARCH, ...overrides }}
      />
    </I18nextProvider>,
  );
}

describe("My library", () => {
  it("puts the newest arrival first", () => {
    const older: LibraryTitleDto = {
      id: 1,
      name: "Older Arrival",
      kind: "movies",
      stub: false,
      items: 1,
      files: 1,
      bytes: 1,
      missingFiles: 0,
      addedAt: 1,
      overview: null,
      rating: null,
      poster: null,
      needsPoster: false,
    };
    const newer: LibraryTitleDto = { ...older, id: 2, name: "Newer Arrival", addedAt: 2 };

    const html = renderToStaticMarkup(
      <I18nextProvider i18n={i18n}>
        <DiscoverView
          libraryTitles={[older, newer]}
          onNavigate={() => undefined}
          rails={railsView()}
          search={{ ...NO_SEARCH }}
        />
      </I18nextProvider>,
    );

    expect(html.indexOf("Newer Arrival")).toBeLessThan(html.indexOf("Older Arrival"));
    // The tile carries the same line a provider rail's tile does, rather than a bare item count.
    expect(html).toContain("Film · 1 item");
  });

  it("gives a library tile its title, kind and count, and the watchlist bookmark", () => {
    // **The regression this covers.** The library rail once drew a bare item count: no kind, no
    // overview, and no mark for a title on the account's list, while the provider rails beside it
    // carried all three. A static render cannot hover a tile, so the title and meta are asserted
    // where a screen reader reads them — the tile's accessible name — and the bookmark's word is
    // asserted there too, because that is what keeps the mark from being colour alone.
    const watched: LibraryTitleDto = {
      id: 2,
      name: "Newer Arrival",
      kind: "shows",
      stub: false,
      items: 2,
      files: 3,
      bytes: 1,
      missingFiles: 0,
      addedAt: 2,
      overview: null,
      rating: null,
      poster: null,
      needsPoster: false,
    };
    const unwatched: LibraryTitleDto = { ...watched, id: 3, name: "Other Arrival", addedAt: 1 };
    const saved: WatchlistEntryDto[] = [
      { titleId: 2, title: "Newer Arrival", poster: null, kind: "show", addedAt: 0 },
    ];

    const html = renderToStaticMarkup(
      <I18nextProvider i18n={i18n}>
        <DiscoverView
          libraryTitles={[watched, unwatched]}
          onNavigate={() => undefined}
          rails={railsView()}
          search={{ ...NO_SEARCH }}
          watchlist={saved}
        />
      </I18nextProvider>,
    );

    // One name carries all three facts, which is also how a screen reader meets the tile: the
    // title, the watchlist word, then the same kind/count line a provider rail's tile carries.
    expect(html).toContain("Newer Arrival, On My list, Series · 2 items");
    // And the tile that is not on the list says so by omission: no mark, no word, same meta.
    expect(html).toContain("Other Arrival, Series · 2 items");
    expect(html).not.toContain("Other Arrival, On My list");
  });
});

describe("Continue Watching", () => {
  it("narrows to the selected type, like every other rail", () => {
    const show: ContinueWatchingDto = {
      titleId: 16,
      title: "VisionQuest",
      poster: null,
      kind: "show",
      episodeId: 2298,
      seasonNumber: 1,
      episodeNumber: 1,
      positionMs: 60_000,
      durationMs: 2_400_000,
      percent: 0.025,
      updatedAt: 1,
    };
    const film: ContinueWatchingDto = {
      ...show,
      titleId: 17,
      title: "The Long Goodbye",
      kind: "movie",
      episodeId: null,
      seasonNumber: null,
      episodeNumber: null,
    };

    const html = renderToStaticMarkup(
      <I18nextProvider i18n={i18n}>
        <DiscoverView
          continueWatching={{ rows: [show, film], loading: false, failed: false }}
          onNavigate={() => undefined}
          rails={railsView()}
          search={{ ...NO_SEARCH }}
          titleKind="movie"
        />
      </I18nextProvider>,
    );

    expect(html).toContain("The Long Goodbye");
    expect(html).not.toContain("VisionQuest");
  });

  it("says the filter hid the section rather than drawing it empty", () => {
    const show: ContinueWatchingDto = {
      titleId: 16,
      title: "VisionQuest",
      poster: null,
      kind: "show",
      episodeId: 2298,
      seasonNumber: 1,
      episodeNumber: 1,
      positionMs: 60_000,
      durationMs: 2_400_000,
      percent: 0.025,
      updatedAt: 1,
    };

    const html = renderToStaticMarkup(
      <I18nextProvider i18n={i18n}>
        <DiscoverView
          continueWatching={{ rows: [show], loading: false, failed: false }}
          onNavigate={() => undefined}
          rails={railsView()}
          search={{ ...NO_SEARCH }}
          titleKind="movie"
        />
      </I18nextProvider>,
    );

    expect(html).toContain("Continue Watching");
    expect(html).toContain("Nothing here for this filter");
  });
});

describe("the type filter", () => {
  it("narrows My library and My list to the selected type too", () => {
    const film: LibraryTitleDto = {
      id: 1,
      name: "A Film",
      kind: "movies",
      stub: false,
      items: 1,
      files: 1,
      bytes: 1,
      missingFiles: 0,
      addedAt: 2,
      overview: null,
      rating: null,
      poster: null,
      needsPoster: false,
    };
    const series: LibraryTitleDto = { ...film, id: 2, name: "A Series", kind: "shows" };
    const saved: WatchlistEntryDto[] = [
      { titleId: 1, title: "A Film", poster: null, kind: "movie", addedAt: 1 },
      { titleId: 2, title: "A Series", poster: null, kind: "show", addedAt: 0 },
    ];

    const html = renderToStaticMarkup(
      <I18nextProvider i18n={i18n}>
        <DiscoverView
          libraryTitles={[film, series]}
          onNavigate={() => undefined}
          rails={railsView()}
          search={{ ...NO_SEARCH }}
          titleKind="show"
          watchlist={saved}
        />
      </I18nextProvider>,
    );

    expect(html).toContain("A Series");
    expect(html).not.toContain("A Film");
  });
});

describe("DiscoverView", () => {
  it("draws a rail's heading and one tile per title", () => {
    const html = markup(
      railsView({
        rails: [
          rail({
            items: [
              item({ titleId: 1, name: "The Matrix" }),
              item({ titleId: 2, name: "Inception", year: 2010 }),
            ],
          }),
        ],
      }),
    );

    expect(html).toContain("Popular films");
    expect(html).toContain("/title/movie/1");
    expect(html).toContain("/title/movie/2");
    // The accessible name a screen reader reads; the tile itself carries no visible caption.
    expect(html).toContain("The Matrix, 1999 · Film");
  });

  it("draws the score a rail item carries, and no badge when it carries none", () => {
    // The badge is `TitleCard`'s and is not this screen's to change; what this owns is that the
    // rating the core answered with reaches the card, and that an item without one draws nothing
    // rather than a zero the provider never sent.
    const scored = markup(railsView({ rails: [rail({ items: [item({ rating: 8.2 })] })] }));
    expect(scored).toContain("8.2");

    const unscored = markup(railsView({ rails: [rail({ items: [item({ rating: null })] })] }));
    expect(unscored).not.toContain("8.2");
  });

  it("writes a rail item's score into the tile's own name, and no count", () => {
    // The hover preview's panel waits for a pointer this environment does not have, so the part of it
    // a static render reaches is the accessible name — which is where "8.2" is written for a screen
    // reader, and where a caller's mapping from the core's number to words either exists or does not.
    // The count is asserted out: it is off the tiles and off the hero, and a reintroduction is a
    // failure rather than a detail.
    const html = markup(
      railsView({
        rails: [
          rail({
            items: [
              item({
                overview: "A hacker learns the truth.",
                rating: 8.2,
                votes: 24_000,
              }),
            ],
          }),
        ],
      }),
    );

    expect(html).toContain("The Matrix, 1999 · Film, rated 8.2 out of 10");
    expect(html).not.toContain("24,000");
    expect(html).not.toContain("votes");
  });

  it("gives every rail a heading and its own empty state, fetched or not", () => {
    // The geometry rule: a rail nobody has fetched yet is an empty rail rather than an absent one,
    // so the headings below it cannot move when the fetch lands.
    const html = markup(
      railsView({ rails: [rail({ rail: "on_the_air_shows", items: [] }), rail()] }),
    );

    expect(html).toContain("On the air");
    expect(html).toContain("Popular films");
    expect(html).toContain("Nothing in this rail yet");
    expect(html).toContain("Refresh to ask the providers, or add a provider key in Settings.");
  });

  it("features the first rail that has items as the hero, and rotates through it", () => {
    const html = markup(
      railsView({
        rails: [
          rail({ rail: "trending_week", items: [] }),
          rail({
            rail: "popular_movies",
            items: [item({ titleId: 9, name: "Dune" }), item({ titleId: 10, name: "Arrival" })],
          }),
        ],
      }),
    );

    expect(html).toContain("Featured title");
    expect(html).toContain("View More");
    expect(html).toContain("/title/movie/9");
    expect(html).toContain("Dune");
    expect(html).toContain("1999 · Film");
    // Both slides are in the DOM at once, which is what the cross-fade needs: the incoming image
    // cannot fade in if it arrives with the class that shows it.
    expect(html).toContain("opacity-100");
    expect(html).toContain("opacity-0");
    expect(html).toContain('aria-label="Show Arrival"');
  });

  it("puts search and category controls inside the hero overlay", () => {
    // The reference's order, and the reason it matters: a box at the top whose results appeared
    // below the rails would be a box whose results nobody finds.
    const html = markup(railsView());

    expect(html.indexOf("<form")).toBeGreaterThan(html.indexOf("Featured title"));
    expect(html).toContain("absolute inset-x-0 top-0 z-20");
    expect(html).toContain("data-discover-toolbar");
    expect(html).toContain("Movies");
    expect(html).toContain("TV shows");
    expect(html).toContain("bg-surface-0/70");
    expect(html.indexOf("<form")).toBeLessThan(html.indexOf("Popular films"));
  });

  it("says what to do when nothing has been fetched, and still draws no spinner", () => {
    const html = markup(railsView({ rails: [] }));

    expect(html).toContain("Nothing here yet");
    expect(html).toContain("Settings");
    expect(html).not.toContain("animate-spin");
    expect(html).not.toContain('role="progressbar"');
  });

  it("asks for a key when the core says no provider is configured", () => {
    const html = markup(
      railsView({ rails: [rail({ items: [], state: "noProvider" })], state: "noProvider" }),
    );

    expect(html).toContain("Nothing here yet");
    expect(html).toContain("a provider key is set up in Settings");
    expect(html).not.toContain("Fetching the rails");
  });

  it("says the rails are being fetched rather than asking for a key the user has set up", () => {
    // Two rails, both empty and both saying the same thing: the state is one fact about the read, so
    // whichever rail the screen reads it from, the sentence is the same.
    const html = markup(
      railsView({
        rails: [
          rail({ rail: "trending_week", items: [], state: "fetching" }),
          rail({ items: [], state: "fetching" }),
        ],
        state: "fetching",
      }),
    );

    expect(html).toContain("Fetching the rails from your provider");
    expect(html).not.toContain("Nothing here yet");
    expect(html).not.toContain("add a provider key in Settings");
    // A sentence in the section's own line, plus the placeholders below it — never a spinner
    // (§11 rule 2). The placeholders are the poster tiles' own shape, which is what makes the answer
    // arriving a fill rather than a re-layout.
    expect(html.match(/aspect-ratio/gu)?.length ?? 0).toBeGreaterThan(0);
    expect(html).toContain('aria-busy="true"');
    expect(html).not.toContain("animate-spin");
    expect(html).not.toContain('role="progressbar"');
  });

  it("claims nothing about the catalogue while a read of a new selection is in flight", () => {
    // The Movies / TV shows switch re-reads the core, and the hook answers with no rails until it
    // lands. That is one IPC round trip: too short for placeholders, and long enough that "Nothing
    // here yet" would be a claim about a catalogue nobody has read yet.
    const html = markup(railsView({ loading: true, rails: [] }));

    expect(html).not.toContain("Nothing here yet");
    expect(html).toContain("A film or a series");
  });

  it("keeps the posters it has while a refresh runs instead of trading them for placeholders", () => {
    // A rail that already answered keeps drawing what it has: stale posters are more use than
    // placeholders, and the loading state is for a rail with nothing to show.
    const html = markup(
      railsView({
        rails: [rail({ items: [item({ titleId: 4 })], state: "ready" })],
        refreshing: true,
        state: "ready",
      }),
    );

    expect(html).toContain("/title/movie/4");
    expect(html).not.toContain("aspect-ratio");
  });

  it("draws the hero's band, with the toolbar on it, while the first fetch is in the air", () => {
    // The band is the hero's own box: without it the toolbar would sit alone and everything below it
    // would move the moment the first title arrived. With no rails on the way there is no band — the
    // screen keeps its empty state rather than a placeholder for content nobody is fetching.
    const fetching = markup(railsView({ rails: [], state: "fetching" }));
    expect(fetching).toContain("min-h-[28rem]");
    expect(fetching).toContain("A film or a series");

    const idle = markup(railsView({ rails: [] }));
    expect(idle).not.toContain("min-h-[28rem]");
    expect(idle).toContain("A film or a series");
  });

  it("draws the rails once the providers have answered", () => {
    const html = markup(
      railsView({
        rails: [rail({ items: [item({ titleId: 4 })], state: "ready" })],
        state: "ready",
      }),
    );

    expect(html).toContain("Popular films");
    expect(html).toContain("/title/movie/4");
    expect(html).not.toContain("Fetching the rails");
    expect(html).not.toContain("Nothing here yet");
  });

  it("keeps the search box, which is what a person came to type in", () => {
    const html = markup(railsView());

    expect(html).toContain("<form");
    expect(html).toContain("Search for a title");
  });

  it("offers a Refresh control in both of its states", () => {
    expect(markup(railsView())).toContain("Refresh recommendations");
    expect(markup(railsView({ refreshing: true }))).toContain("Refreshing recommendations…");
  });

  it("reports a failed refresh without discarding the rails that did arrive", () => {
    const error = { code: "INTERNAL", message: "the scheduler stopped" } as IpcError;
    const html = markup(railsView({ refreshError: error }));

    expect(html).toContain("The refresh could not be started.");
    expect(html).toContain("Popular films");
    expect(html).toContain("/title/movie/7");
  });

  it("distinguishes a read that failed from a catalogue that is empty", () => {
    const html = markup(railsView({ rails: [], failed: true }));

    expect(html).toContain("The rails could not be read");
    expect(html).not.toContain("Nothing here yet");
  });

  it("names itself once, with a first-level heading", () => {
    expect(markup(railsView()).match(/<h1/gu)).toHaveLength(1);
  });
});

describe("the hero's slides and the tile caption", () => {
  it("takes the first rail that has items, in order, capped", () => {
    // The rule the screen and the core share (`hero_backdrops`), asserted here as the screen uses it.
    expect(heroSlides([])).toEqual([]);
    expect(heroSlides([rail({ items: [] })])).toEqual([]);
    expect(heroSlides([rail({ items: [item({ titleId: 3 })] })])[0]?.titleId).toBe(3);
    expect(
      heroSlides([
        rail({ items: [] }),
        rail({ items: [item({ titleId: 4 }), item({ titleId: 5 })] }),
      ]).map((slide) => slide.titleId),
    ).toEqual([4, 5]);
  });

  it("writes a year it has and stays honest when it does not", () => {
    // A tile whose caption invented a year would be a claim the provider never made.
    expect(itemMeta(item({ year: 1999 }), "Film")).toBe("1999 · Film");
    expect(itemMeta(item({ year: null }), "Series")).toBe("Series");
  });
});

describe("where a search answer came from", () => {
  /** One title a search matched, which is what the panel lists. */
  const hit: TitleMatchDto = { id: 1, name: "The Matrix", kind: "movies" };

  it("says the results are the catalogue's own when the local index answered", () => {
    // The whole point of searching the catalogue first: a reader who already has a title should be able
    // to tell that no request was spent, and that this is something they can open right now.
    const html = searched({ results: [hit], searched: true, source: "catalogue" });

    expect(html).toContain("In your catalogue");
    expect(html).not.toContain("From your providers");
    expect(html).toContain("The Matrix");
  });

  it("says the results came from a provider when the catalogue had nothing", () => {
    const html = searched({ results: [hit], searched: true, source: "provider" });

    expect(html).toContain("From your providers");
    expect(html).not.toContain("In your catalogue");
  });

  it("claims no source at all when there is nothing to have come from", () => {
    // Neither line belongs on an empty answer: "nothing in your catalogue" is what the empty state says,
    // and a source label above it would be attributing a result that does not exist.
    const html = searched({ results: [], searched: true, source: "none" });

    expect(html).not.toContain("In your catalogue");
    expect(html).not.toContain("From your providers");
  });
});

describe("the search's live status", () => {
  const hit: TitleMatchDto = { id: 1, name: "The Matrix", kind: "movies" };

  it("says a read is in the air, then how many titles arrived", () => {
    // The panel under the field appears without focus moving, so the count is what tells a
    // screen-reader user that the debounce has settled and an answer is on screen.
    expect(searched({ searching: true })).toMatch(/role="status"[^>]*>Searching the providers…</u);
    expect(searched({ results: [hit], searched: true, source: "catalogue" })).toMatch(
      /role="status"[^>]*>1 result</u,
    );
    expect(searched({ results: [], searched: true, source: "none" })).toMatch(
      /role="status"[^>]*>Nothing matched</u,
    );
  });
});
