import { createI18n } from "@peregrine/i18n";
import type { EpisodeDto, LibraryTitleDto } from "@peregrine/ipc-client";
import { renderToStaticMarkup } from "react-dom/server";
import { I18nextProvider } from "react-i18next";
import { describe, expect, it } from "vitest";
import type { TitleDetail } from "../hooks/useTitle.js";
import { TitleScreen } from "./TitleScreen.js";

const i18n = createI18n();

function title(overrides: Partial<LibraryTitleDto> = {}): LibraryTitleDto {
  return {
    id: 7,
    name: "Breaking Bad",
    kind: "shows",
    stub: true,
    items: 3,
    files: 3,
    bytes: 12 * 1024 * 1024 * 1024,
    missingFiles: 1,
    ...overrides,
  };
}

function episode(overrides: Partial<EpisodeDto> = {}): EpisodeDto {
  return {
    seasonNumber: 1,
    episodeNumber: 2,
    name: null,
    items: 1,
    files: 1,
    bytes: 2 * 1024 * 1024 * 1024,
    missingFiles: 0,
    ...overrides,
  };
}

function detail(state: Partial<TitleDetail> = {}): TitleDetail {
  return {
    title: title(),
    episodes: [],
    loading: false,
    missing: false,
    failed: false,
    refresh: () => Promise.resolve(),
    ...state,
  };
}

/** The screen as markup, which is what its two reads produce. */
function markup(state: TitleDetail = detail(), kind: "movie" | "show" = "show"): string {
  return renderToStaticMarkup(
    <I18nextProvider i18n={i18n}>
      <TitleScreen detail={state} kind={kind} />
    </I18nextProvider>,
  );
}

describe("TitleScreen", () => {
  it("names itself from the route before anything has been read", () => {
    // PG-E7-02's acceptance, on the one route that has a parameter: the kind is in the path, so the
    // page says what it is while the read is still in flight rather than being a full-page wait.
    const loading = markup(detail({ title: null, loading: true }), "movie");

    expect(loading).toContain("Film");
    expect(loading).not.toContain("not here");
  });

  it("shows the totals the library holds", () => {
    const html = markup(detail({ title: title({ items: 3, files: 4, missingFiles: 1 }) }));

    expect(html).toContain("Breaking Bad");
    expect(html).toContain("In your library");
    expect(html).toContain("4");
  });

  it("lists a series' episodes in watch order, with the ones nobody named yet", () => {
    // An episode with no provider metadata is the normal state of a fresh scan, and saying so is
    // better than an empty cell: the row exists and the name is what is missing.
    const html = markup(
      detail({
        episodes: [
          episode({ seasonNumber: 1, episodeNumber: 1 }),
          episode({ name: "Cat's in the Bag" }),
        ],
      }),
    );

    expect(html).toContain("Episodes");
    expect(html).toContain("1×01");
    expect(html).toContain("Not named yet");
    expect(html).toContain("Cat&#x27;s in the Bag");
  });

  it("asks a film for no episodes at all", () => {
    // A film has no episodes, so the section is not drawn — an empty "Episodes" heading under a film
    // would be a question the screen already knows the answer to.
    const html = markup(detail({ title: title({ kind: "movies", name: "Alien" }) }), "movie");

    expect(html).toContain("Alien");
    expect(html).not.toContain("Episodes");
  });

  it("says a title is not here rather than failing", () => {
    // `null` covers both "out of scope" and "no such id" (I37), and a page that reported a failure
    // would be telling the user something the core did not say.
    const missing = markup(detail({ title: null, missing: true }));

    expect(missing).toContain("This title is not here.");
    expect(missing).not.toContain("could not be read");
  });

  it("tells a failed read apart from a title that is absent", () => {
    const failed = markup(detail({ title: null, failed: true }));

    expect(failed).toContain("could not be read");
    expect(failed).not.toContain("This title is not here.");
  });
});
