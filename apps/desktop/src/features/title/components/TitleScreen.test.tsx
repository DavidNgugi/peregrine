import { createI18n } from "@peregrine/i18n";
import type {
  DownloadJobDto,
  EpisodeDto,
  LibraryTitleDto,
  RailItemDto,
  SeasonDto,
} from "@peregrine/ipc-client";
import { renderToStaticMarkup } from "react-dom/server";
import { I18nextProvider } from "react-i18next";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { AcquireControl } from "../hooks/useAcquire.js";
import type { TitleDetail } from "../hooks/useTitle.js";
import type { TitleSeasons } from "../hooks/useSeasons.js";
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
    addedAt: 0,
    overview: null,
    rating: null,
    poster: null,
    needsPoster: false,
    ...overrides,
  };
}

function episode(overrides: Partial<EpisodeDto> = {}): EpisodeDto {
  return {
    seasonNumber: 1,
    episodeNumber: 2,
    name: null,
    overview: null,
    airDate: null,
    runtimeMinutes: null,
    rating: null,
    still: null,
    items: 1,
    files: 1,
    bytes: 2 * 1024 * 1024 * 1024,
    missingFiles: 0,
    watched: false,
    progress: null,
    ...overrides,
  };
}

/** One queue row, with the pack shape the plan reads when it names no episode. */
function job(overrides: Partial<DownloadJobDto> = {}): DownloadJobDto {
  return {
    id: 1,
    titleId: 7,
    title: "Breaking Bad",
    titleKind: "show",
    poster: null,
    seasonNumber: 1,
    episodeNumber: null,
    releaseTitle: "Breaking.Bad.S01.1080p",
    state: "downloading",
    protocol: "torrent",
    stage: "downloading",
    progressPermille: 0,
    reportedSizeBytes: null,
    downloadedBytes: 0,
    downloadRateBps: 0,
    peersConnected: 0,
    peersSwarm: 0,
    etaSeconds: null,
    addedAt: 0,
    errorCode: null,
    errorMessage: null,
    findings: [],
    hasMediaFile: false,
    ...overrides,
  };
}

/** One season, as the switcher reads them. */
function season(overrides: Partial<SeasonDto> = {}): SeasonDto {
  return {
    seasonNumber: 1,
    name: null,
    episodeCount: null,
    airDate: null,
    poster: null,
    ...overrides,
  };
}

/** The season section's own state, which the screen draws separately from the title's. */
function seasons(state: Partial<TitleSeasons> = {}): TitleSeasons {
  return {
    seasons: [season()],
    season: season(),
    episodes: [],
    loading: false,
    failed: null,
    choose: () => undefined,
    setEpisodeWatched: async () => undefined,
    setSeasonWatched: async () => undefined,
    markFailed: null,
    marking: false,
    ...state,
  };
}

function detail(state: Partial<TitleDetail> = {}): TitleDetail {
  return {
    title: title(),
    // Unknown rather than absent: this fixture is about what the page draws, and a null container is
    // the state a screen cannot reach from a successful read of a title the library holds.
    cast: [],
    playSupport: "webview",
    watched: false,
    poster: null,
    backdrop: null,
    overview: null,
    rating: null,
    released: null,
    // Playable, and the reason is the fixture's own counts: three files, one of them missing, is a
    // copy on disk. The offer is the core's answer rather than the counts' — see `lib/play-action.ts`.
    offer: "play",
    loading: false,
    missing: false,
    failed: false,
    // No refresh watched stop: this fixture is drawn from one read, and the season read a stop would
    // re-run has its own fixture.
    refreshes: 0,
    updates: 0,
    seasonUpdates: 0,
    refreshing: false,
    refresh: () => Promise.resolve(),
    ...state,
  };
}

/** The screen as markup, which is what its reads produce. */
function markup(
  state: TitleDetail = detail(),
  kind: "movie" | "show" = "show",
  similar: readonly RailItemDto[] = [],
  seasonState: TitleSeasons = seasons(),
): string {
  return renderToStaticMarkup(
    <I18nextProvider i18n={i18n}>
      <TitleScreen
        detail={state}
        feed={{
          items: similar,
          loading: false,
          failed: null,
          exhausted: true,
          loadMore: () => undefined,
        }}
        kind={kind}
        onNavigate={() => undefined}
        seasons={seasonState}
      />
    </I18nextProvider>,
  );
}

/** The acquisition control as a page holds it, with the write recorded rather than performed. */
function acquireControl(overrides: Partial<AcquireControl> = {}): AcquireControl {
  return {
    starting: false,
    running: false,
    phase: null,
    permille: 0,
    outcome: null,
    findings: [],
    failure: null,
    error: null,
    titleId: 7,
    episode: null,
    start: async () => undefined,
    ...overrides,
  };
}

/** The screen with the acquisition control drawn, which the season button needs. */
function seasonMarkup(
  seasonState: TitleSeasons,
  downloadRows: readonly DownloadJobDto[] = [],
): string {
  return renderToStaticMarkup(
    <I18nextProvider i18n={i18n}>
      <TitleScreen
        acquire={acquireControl()}
        detail={detail()}
        downloadRows={downloadRows}
        feed={{
          items: [],
          loading: false,
          failed: null,
          exhausted: true,
          loadMore: () => undefined,
        }}
        kind="show"
        onNavigate={() => undefined}
        seasons={seasonState}
      />
    </I18nextProvider>,
  );
}

/** One recommendation tile, which is the shape a rail item is. */
function recommendation(overrides: Partial<RailItemDto> = {}): RailItemDto {
  return {
    titleId: 12,
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

describe("TitleScreen", () => {
  it("names itself from the route before anything has been read", () => {
    // PG-E7-02's acceptance, on the one route that has a parameter: the kind is in the path, so the
    // page says what it is while the read is still in flight rather than being a full-page wait.
    const loading = markup(detail({ title: null, loading: true }), "movie");

    expect(loading).toContain("Film");
    expect(loading).not.toContain("not here");
  });

  it("says a refresh is in flight and draws the cast's geometry", () => {
    const html = markup(detail({ refreshing: true }), "show");

    expect(html).toContain("Fetching details, cast, episodes, and artwork…");
    expect(html).toContain("Cast");
  });

  it("says how much space the title takes rather than drawing a table of counts", () => {
    const html = markup(detail({ title: title({ items: 3, files: 4, missingFiles: 0 }) }));

    expect(html).toContain("Breaking Bad");
    expect(html).toContain("In your library");
    expect(html).toContain("12 GB");
    // The counts table is gone: a single row of items, copies, size, and a dash was a readout.
    expect(html).not.toContain("Copies");
  });

  it("says how many of the title's files the library cannot find", () => {
    const one = markup(detail({ title: title({ files: 4, missingFiles: 1 }) }));
    const many = markup(detail({ title: title({ files: 4, missingFiles: 3 }) }));

    expect(one).toContain("A file for this title is missing.");
    expect(many).toContain("3 files for this title are missing.");
  });

  it("says nothing about missing files when every one of them is present", () => {
    const html = markup(detail({ title: title({ missingFiles: 0 }) }));

    expect(html).not.toContain("for this title is missing");
    expect(html).not.toContain("for this title are missing");
  });

  it("leaves the fully missing title to the hero's own sentence", () => {
    // 'absent' is the core's answer for a title with a row and no file it can see; the hero already
    // says that in the control's reason, so the count would be the same fact twice.
    const html = markup(detail({ title: title({ files: 3, missingFiles: 3 }), offer: "absent" }));

    expect(html).toContain("The library recorded a file for this title");
    expect(html).not.toContain("for this title are missing");
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

  it("draws the hero: the backdrop behind the poster, the facts, the score and the synopsis", () => {
    bridge.inTauri = true;
    const html = markup(
      detail({
        poster: "/cache/images/poster.webp",
        backdrop: "/cache/images/backdrop.webp",
        released: "2021-04-30",
        rating: 8.1,
        overview: "Two men are pulled into a photograph.",
      }),
      "show",
      [],
      seasons({
        seasons: [season({ seasonNumber: 1 }), season({ seasonNumber: 2 })],
        episodes: [episode()],
      }),
    );

    // Both pictures, each through the asset protocol and encoded: the backdrop is the wide one behind
    // the name and the poster is the portrait beside it, and neither is bytes on the wire (R11).
    expect(html).toContain("asset://localhost/%2Fcache%2Fimages%2Fbackdrop.webp");
    expect(html).toContain("asset://localhost/%2Fcache%2Fimages%2Fposter.webp");
    // The scrim is what makes the name legible over an arbitrary photograph, so its absence is a
    // contrast bug rather than a cosmetic one.
    expect(html).toContain("bg-gradient-to-b");
    expect(html).toContain("8.1");
    expect(html).toContain("2021-04-30");
    expect(html).toContain("2 seasons");
    expect(html).toContain("Two men are pulled into a photograph.");
  });

  it("omits every hero fact nobody stated rather than drawing an empty one", () => {
    // A title no provider has filled in is the ordinary state of a fresh scan, and a row of dashes for
    // facts nobody stated is noise on every such page.
    const html = markup(
      detail({ title: title({ kind: "movies", items: 0 }) }),
      "movie",
      [],
      // A film asks for no seasons at all, which is the state the hook short-circuits to.
      seasons({ seasons: [], season: null }),
    );

    expect(html).toContain("Play");
    // No counts, because there are none to state: a film has no seasons, and the library holds nothing.
    expect(html).not.toContain("1 season");
    expect(html).not.toContain("episode</span>");
    // The score badge is absent rather than zero: a `0.0` would be a claim the provider never made.
    expect(html).not.toContain("0.0");
  });

  it("offers a working Play with no Download control at all for a title the library holds", () => {
    // **The report this test exists for.** A copy is on disk, so Play is the action and Download is not
    // drawn in either state: a disabled Download beside a working Play still reads as an offer to fetch
    // what the user already has.
    //
    // **And Play works now.** The reason sentence is gone and the control is live, because the surface
    // ADR 0023 decides exists — the page hands the hero a handler and the core classified the container
    // as one this build reads.
    //
    // **`disabled=""` and not the word `disabled`.** Every button in the kit carries
    // `disabled:pointer-events-none` in its class list, so a search for the bare word counts buttons
    // rather than inert ones — which is how the first version of this assertion passed while measuring
    // nothing. The attribute is what React renders when the prop is true.
    const held = markup(detail({ offer: "play" }), "show");

    expect(held).toContain("Play");
    expect(held).not.toContain("no player yet");
    expect(held).not.toContain("Download");
    expect(held).not.toContain("Acquiring is not built yet");
    expect(held).not.toContain(`disabled=""`);
  });

  it("routes a container the page cannot read to the external player", () => {
    // The tenth file in this project's own library is HEVC inside Matroska, which a WebKit `<video>`
    // cannot demux. The Play action starts mpv instead, and the page says so beside the control.
    const mkv = markup(detail({ offer: "play", playSupport: "externalPlayer" }), "show");

    expect(mkv).toContain("Opens in mpv");
    expect(mkv).not.toContain("cannot read that file&#x27;s format");
    expect(mkv).not.toContain("Download");
  });

  it("plays a container it cannot name, and says so rather than promising it", () => {
    const odd = markup(detail({ offer: "play", playSupport: "unknown" }), "show");

    expect(odd).toContain("does not recognise this file&#x27;s format");
    expect(odd).not.toContain("cannot read that file&#x27;s format");
  });

  it("offers acquisition, and only acquisition, for a title the library holds nothing of", () => {
    // `PG-E1-07`: a control this build cannot honour is disabled with the reason it is. The fixture
    // hands the screen no acquisition handler, so both controls are disabled — Play because there is
    // no file, Download because there is nothing to start.
    const absent = markup(detail({ offer: "acquire", title: title({ items: 0 }) }), "show");

    expect(absent).toContain("holds no file");
    expect(absent).toContain("Download");
    expect(absent).toContain("will find the best release");
    expect(absent.match(/<button[^>]*disabled/gu)?.length).toBe(2);
  });

  it("says a recorded file is not on disk rather than claiming the library never had one", () => {
    // `absent` is a title with a row and no bytes the library can see — an unplugged drive, a file
    // moved behind the app. The sentence a reader needs is about that file, not about a title nobody
    // has, and acquisition is not offered: asking again would record a second copy of a file the
    // library already knows about.
    const gone = markup(detail({ offer: "absent" }), "show");

    expect(gone).toContain("not on disk right now");
    expect(gone).not.toContain("holds no file");
    expect(gone).not.toContain("Download");
  });

  it("waits for a running transfer that has not named its file yet", () => {
    // A show's hero has no title-level file to name: support is null until the episode a person
    // picked is the thing being played.
    const arriving = markup(detail({ offer: "arriving", playSupport: null }), "show");

    expect(arriving).toContain("in flight");
    expect(arriving).not.toContain("Download");
    expect(arriving.match(/<button[^>]*disabled/gu)?.length).toBe(1);
  });

  it("offers Schedule and no watch mark for a film that is not out", () => {
    const html = markup(detail({ offer: "acquire", released: "2999-01-01" }), "movie");

    expect(html).toContain("Schedule");
    expect(html).not.toContain("Mark watched");
    expect(html).not.toContain("Download");
  });

  it("plays a film whose download has named its file", () => {
    const arriving = markup(detail({ offer: "arriving", playSupport: "webview" }), "movie");

    expect(arriving).toContain("Playing while it downloads.");
    expect(arriving).not.toContain("in flight");
    expect(arriving).not.toContain("Download");
  });

  it("draws one card per episode of the chosen season, each with its number and name", () => {
    // A card rather than a row: what a reader scans is a still, a number, and a name, and an episode
    // nobody has named yet says so rather than drawing an empty cell.
    const html = markup(
      detail(),
      "show",
      [],
      seasons({
        episodes: [
          episode({ seasonNumber: 1, episodeNumber: 1 }),
          episode({ name: "Cat's in the Bag" }),
        ],
      }),
    );

    expect(html).toContain("Episodes");
    expect(html).toContain("1×01");
    expect(html).toContain("1×02");
    expect(html).toContain("Not named yet");
    expect(html).toContain("Cat&#x27;s in the Bag");
    // One card each, and the card is the only thing an episode draws: two `<article>`s for two
    // episodes is the count a reader sees, and it is what a table of rows used to be.
    expect(html.match(/<article/g)).toHaveLength(2);
  });

  it("gives every episode card one size, whatever each episode's metadata holds", () => {
    // A provider dates and describes some episodes and not others, so two cards in one row carry
    // different numbers of facts — and a grid whose rows size themselves to their own content staggers,
    // which reads as a broken grid rather than as missing metadata. Three things together are what make
    // the cards one size, and class assertions are the most a markup test can make of them:
    // `auto-rows-fr` on the grid, `h-full` on every card, and a clamp on the synopsis so a paragraph
    // cannot set the height of every card in the season on its own.
    const html = markup(
      detail(),
      "show",
      [],
      seasons({
        episodes: [
          episode({
            seasonNumber: 1,
            episodeNumber: 1,
            airDate: "2008-01-20",
            runtimeMinutes: 58,
            overview: "A chemistry teacher gets a diagnosis.",
          }),
          // The other extreme: every optional fact absent, which is the shape that used to draw a
          // visibly shorter card beside the first.
          episode({
            seasonNumber: 1,
            episodeNumber: 2,
            airDate: null,
            runtimeMinutes: null,
            overview: null,
          }),
        ],
      }),
    );

    expect(html).toMatch(/<ul[^>]*class="[^"]*auto-rows-fr/u);
    // Both cards, not just the one that happens to be tall: one `h-full` would let the other collapse,
    // which is the failure this test exists to catch.
    expect(html.match(/<article[^>]*class="[^"]*\bh-full\b/gu)).toHaveLength(2);
    expect(html).toMatch(/<p class="[^"]*line-clamp-3[^"]*">A chemistry teacher/u);
  });

  it("lists the seasons in order and marks the one on screen as pressed", () => {
    const html = markup(
      detail(),
      "show",
      [],
      seasons({
        seasons: [
          season({ seasonNumber: 1 }),
          season({ seasonNumber: 2 }),
          season({ seasonNumber: 0, name: "Specials" }),
        ],
        season: season({ seasonNumber: 2 }),
        episodes: [episode({ seasonNumber: 2, episodeNumber: 1 })],
      }),
    );

    const chips = [...html.matchAll(/aria-pressed="(true|false)"[^>]*>([^<]+)</gu)].map((m) => [
      m[2],
      m[1],
    ]);
    expect(chips).toEqual([
      ["Season 1", "false"],
      ["Season 2", "true"],
      ["Specials", "false"],
    ]);
    // Every chip carries the kit's focus ring rather than the webview's default: the chip is a raw
    // button, so the visible focus state is this screen's to own.
    expect(html).toMatch(
      /<button aria-pressed="(true|false)" class="[^"]*focus-visible:outline-2/u,
    );
  });

  it("draws the reserved box for an episode with no still, and never a broken image", () => {
    // The still is asked for through the asset protocol, which is absent outside the shell and refuses
    // a path it does not hold: both land on the placeholder rather than on a broken-image glyph.
    const html = markup(
      detail(),
      "show",
      [],
      seasons({
        episodes: [episode({ still: null })],
      }),
    );

    expect(html).toContain("No still yet");
    expect(html).not.toContain("<img");

    // And when the shell is there, the path the core named is what the card loads — through the asset
    // protocol and encoded, rather than as a raw path the webview would resolve against its own
    // document. `AGENTS R11`: the path is the whole of what crosses IPC.
    bridge.inTauri = true;
    const withStill = markup(
      detail(),
      "show",
      [],
      seasons({
        episodes: [episode({ still: "/cache/images/still.webp" })],
      }),
    );

    expect(withStill).not.toContain("No still yet");
    expect(withStill).toContain("asset://localhost/%2Fcache%2Fimages%2Fstill.webp");
  });

  it("asks a film for no seasons at all", () => {
    // A film has no seasons, so the section is not drawn — an empty "Episodes" heading under a film
    // would be a question the screen already knows the answer to.
    const html = markup(detail({ title: title({ kind: "movies", name: "Alien" }) }), "movie");

    expect(html).toContain("Alien");
    expect(html).not.toContain("Episodes");
  });

  it("draws no season section for a show whose provider named no seasons", () => {
    // A heading over an empty switcher is a promise nobody made, and it is the state a show nobody has
    // fetched arrives in rather than an error.
    const html = markup(detail(), "show", [], seasons({ seasons: [], season: null }));

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

  it("draws the provider's recommendations under the episodes, each linked to its own page", () => {
    const html = markup(detail(), "movie", [
      recommendation({ titleId: 12, name: "The Matrix Reloaded", year: 2003 }),
      recommendation({ titleId: 13, kind: "show", name: "The Matrix Revolutions", year: null }),
    ]);

    expect(html).toContain("More like this");
    expect(html).toContain("/title/movie/12");
    expect(html).toContain("/title/show/13");
    // The kind and year come from each recommendation rather than from the page's own title, which
    // is the whole reason the rail builds its own meta line.
    expect(html).toContain("The Matrix Reloaded, 2003 · Film");
    expect(html).toContain("The Matrix Revolutions, Series");
  });

  it("draws no rail when the provider recommended nothing", () => {
    // Empty is both "nothing found" and "nobody to ask", and a heading over an empty strip would be a
    // promise the provider did not make.
    const html = markup(detail(), "movie");

    expect(html).not.toContain("More like this");
  });

  it("draws the rail's heading once", () => {
    // A `Section` wrapped around the rail printed "More like this" twice: once as the section's
    // heading and once as the rail's own, which the rail needs because it is also the accessible name
    // of its region. Two identical headings stacked is what the screenshot of this screen showed.
    const html = markup(detail(), "movie", [recommendation()]);

    expect(html.match(/More like this/gu) ?? []).toHaveLength(1);
  });

  it("offers one season control rather than a download button per episode", () => {
    // The season request is one command now: the core chooses a pack or the episodes and fans out
    // inside the job, so the screen must not draw one acquisition per missing episode.
    const html = seasonMarkup(
      seasons({
        episodes: [
          episode({
            seasonNumber: 1,
            episodeNumber: 1,
            items: 0,
            files: 0,
            bytes: 0,
            missingFiles: 0,
          }),
        ],
      }),
    );

    expect(html.match(/Download season 1/gu) ?? []).toHaveLength(1);
  });

  it("shows a live pack's progress on the season control and its cover on every card", () => {
    // The pack row names no episode. Without reading it as the season's own job, the control kept
    // offering "Download season 1" and every card kept saying "Not acquired" while it downloaded.
    const html = seasonMarkup(
      seasons({
        episodes: [
          episode({
            seasonNumber: 1,
            episodeNumber: 1,
            items: 0,
            files: 0,
            bytes: 0,
            missingFiles: 0,
          }),
        ],
      }),
      [job({ episodeNumber: null, state: "downloading", progressPermille: 450 })],
    );

    expect(html).toContain("Season 1 · 1 pack · 45%");
    expect(html).toContain("Downloading with season 1");
    expect(html).not.toContain("Not acquired");
    // Neither control is an offer to start a second acquisition.
    expect(html).toMatch(/<button[^>]*disabled=""[^>]*>Season 1 · 1 pack · 45%<\/button>/u);
    expect(html).toMatch(/<button[^>]*disabled=""[^>]*>Downloading… 45%<\/button>/u);
  });
});

describe("an episode's still", () => {
  const STILL = "/Users/someone/Library/Caches/app.peregrine.desktop/images/still.webp";
  const asset = (path: string): string => `asset://localhost/${encodeURIComponent(path)}`;

  it("draws the still as an image the webview can load, rather than the reserved box", () => {
    // **The one artwork path nothing asserted.** The poster's asset URL has a test above, the hero's
    // backdrop has one, and an episode with *no* still has one — but an episode that *has* a still was
    // never rendered, so the card could have stopped passing it through entirely and this suite would
    // have stayed green.
    //
    // That is not hypothetical, and it is why this test is worth more than its three lines. On the
    // running app the poster, the backdrop and every still drew their placeholders — and every test
    // here passed — because the placeholder is the *designed* answer for a picture that is missing, so
    // a broken chain and a title with no artwork are indistinguishable by eye. The cause turned out to
    // be the harness rather than the product (an `asset://` URL cannot be loaded by a browser, so the
    // component's own on-error fallback did its job), but the suite's silence about a still that exists
    // is a real gap that would have hidden the same symptom for a real reason.
    const html = markup(detail(), "show", [], seasons({ episodes: [episode({ still: STILL })] }));

    expect(html).toContain(`src="${asset(STILL)}"`);
    expect(bridge.convertFileSrc).toHaveBeenCalledWith(STILL);
  });
});
