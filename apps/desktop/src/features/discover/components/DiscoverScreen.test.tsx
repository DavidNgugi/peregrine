import { createI18n } from "@peregrine/i18n";
import type { HomeRailDto, IpcError, RailItemDto } from "@peregrine/ipc-client";
import { renderToStaticMarkup } from "react-dom/server";
import { I18nextProvider } from "react-i18next";
import { describe, expect, it } from "vitest";
import type { HomeRailsView } from "../hooks/useHomeRails.js";
import { DiscoverView, heroItem, itemMeta } from "./DiscoverScreen.js";

const i18n = createI18n();

/**
 * The drawing, from synthetic DTOs.
 *
 * No network, no Tauri bridge, and no store: `DiscoverView` takes what the hooks answered as props,
 * which is the seam that makes the rails, the hero, and every empty state testable in a Node
 * environment. What this file *cannot* do is drive the refresh click — `renderToStaticMarkup` is one
 * pass with no DOM and no event loop, so the click and the `job://state` follow are unobserved here;
 * `rails-follow.test.ts` covers the matching rule, and the shell exercises the hook itself.
 */
function item(overrides: Partial<RailItemDto> = {}): RailItemDto {
  return {
    titleId: 7,
    kind: "movie",
    name: "The Matrix",
    year: 1999,
    // `null` rather than a path: a Node test has no asset protocol, so a poster reference could not
    // become a URL and the markup below is the reserved placeholder the tile draws either way.
    poster: null,
    // `null` rather than a score: most of what the screen draws has no badge, and a test that wanted
    // one says so.
    rating: null,
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
  submit: () => undefined,
  query: "",
  setQuery: () => undefined,
} as const;

function markup(view: HomeRailsView): string {
  return renderToStaticMarkup(
    <I18nextProvider i18n={i18n}>
      <DiscoverView onNavigate={() => undefined} rails={view} search={{ ...NO_SEARCH }} />
    </I18nextProvider>,
  );
}

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

  it("features the first item of the first non-empty rail as the hero", () => {
    const html = markup(
      railsView({
        rails: [
          rail({ rail: "trending_week", items: [] }),
          rail({ rail: "popular_movies", items: [item({ titleId: 9, name: "Dune" })] }),
        ],
      }),
    );

    expect(html).toContain("Featured title");
    expect(html).toContain("Open this title");
    expect(html).toContain("/title/movie/9");
    expect(html).toContain("Dune, 1999 · Film");
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
    // A sentence in the section's own line, not a spinner over geometry that is already drawn.
    expect(html).not.toContain("animate-spin");
    expect(html).not.toContain('role="progressbar"');
  });

  it("draws the rails once the providers have answered", () => {
    const html = markup(
      railsView({ rails: [rail({ items: [item({ titleId: 4 })], state: "ready" })], state: "ready" }),
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
    expect(markup(railsView())).toContain("Refresh");
    expect(markup(railsView({ refreshing: true }))).toContain("Refreshing…");
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

describe("the hero and the tile caption", () => {
  it("answers the first item of the first non-empty rail, or nothing", () => {
    expect(heroItem([])).toBeNull();
    expect(heroItem([rail({ items: [] })])).toBeNull();
    expect(heroItem([rail({ items: [item({ titleId: 3 })] })])?.titleId).toBe(3);
    expect(
      heroItem([rail({ items: [] }), rail({ items: [item({ titleId: 4 }), item({ titleId: 5 })] })])
        ?.titleId,
    ).toBe(4);
  });

  it("writes a year it has and stays honest when it does not", () => {
    // A tile whose caption invented a year would be a claim the provider never made.
    expect(itemMeta(item({ year: 1999 }), "Film")).toBe("1999 · Film");
    expect(itemMeta(item({ year: null }), "Series")).toBe("Series");
  });
});
