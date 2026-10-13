import { createI18n } from "@peregrine/i18n";
import type { EpisodeDto, LibraryTitleDto } from "@peregrine/ipc-client";
import { renderToStaticMarkup } from "react-dom/server";
import { I18nextProvider } from "react-i18next";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { TitleDetail } from "../hooks/useTitle.js";
import { TitleScreen } from "./TitleScreen.js";

// The poster is loaded through Tauri's own bridge, which a Node test does not have. The factory is
// hoisted above the imports, which is why the spies live in `vi.hoisted` rather than beside the test
// — the shape `lib/media.test.ts` uses, so the two cannot disagree about what the bridge is.
const bridge = vi.hoisted(() => ({
  convertFileSrc: vi.fn((path: string) => `asset://localhost/${encodeURIComponent(path)}`),
  inTauri: true,
}));

vi.mock("@tauri-apps/api/core", () => ({
  convertFileSrc: bridge.convertFileSrc,
  isTauri: () => bridge.inTauri,
}));

// Every test starts outside the shell's poster state and with no bridge call recorded, so "the
// protocol was not asked" is an assertion about this test rather than about the one before it.
beforeEach(() => {
  bridge.convertFileSrc.mockClear();
  bridge.inTauri = true;
});

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
    poster: null,
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

  it("shows the cached poster over the asset protocol, and never through IPC", () => {
    // The whole point of the poster path: the core answers with a file in its image cache, the
    // webview loads it over `asset:`, and no bytes cross IPC (AGENTS R11).
    const html = markup(detail({ poster: "/cache/images/9f2c.webp" }));

    expect(html).toContain('alt="Poster for Breaking Bad"');
    expect(html).toContain("asset://localhost/");
    expect(bridge.convertFileSrc).toHaveBeenCalledWith("/cache/images/9f2c.webp");
  });

  it("says there is no poster yet rather than drawing an empty box", () => {
    const html = markup(detail({ poster: null }));

    expect(html).toContain("No poster yet");
    expect(html).not.toContain("<img");
  });

  it("renders the placeholder for a poster reference that is not a cache path", () => {
    // The DTO is untrusted shape at this boundary (AGENTS §4, "Hostile"): a remote URL would be
    // blocked by the strict CSP, and a data URI would be bytes on the wire. Both are no picture
    // rather than an `<img>` that cannot load.
    for (const reference of [
      "https://image.tmdb.org/t/p/w342/poster.jpg",
      "data:image/webp;base64,UklGRg==",
      "../../etc/passwd",
    ]) {
      const html = markup(detail({ poster: reference }));

      expect(html).toContain("No poster yet");
      expect(html).not.toContain("<img");
    }
    expect(bridge.convertFileSrc).not.toHaveBeenCalled();
  });

  it("renders the placeholder outside the Tauri shell, where there is no asset protocol", () => {
    // The component gallery and the browser preview run the same screens with no shell behind them.
    bridge.inTauri = false;
    const html = markup(detail({ poster: "/cache/images/9f2c.webp" }));

    expect(html).toContain("No poster yet");
    expect(html).not.toContain("<img");
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
