import { createI18n } from "@peregrine/i18n";
import type { LibraryTitleDto, MatchRuleDto, UnmatchedFileDto } from "@peregrine/ipc-client";
import { renderToStaticMarkup } from "react-dom/server";
import { I18nextProvider } from "react-i18next";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { LibraryTitles } from "../hooks/useLibraryTitles.js";
import type { ScanControl } from "../hooks/useScan.js";
import type { MatchRules } from "../hooks/useMatchRules.js";
import type { UnmatchedQueue } from "../hooks/useUnmatchedFiles.js";
import type { LibraryView } from "../hooks/useLibraryView.js";
import { LibraryScreen } from "./LibraryScreen.js";

// A tile's poster is loaded through Tauri's own bridge, which a Node test does not have. The factory
// is hoisted above the imports, which is why the spy lives in `vi.hoisted` — the shape
// `TitleScreen.test.tsx` and `lib/media.test.ts` use, so the three cannot disagree about the bridge.
const bridge = vi.hoisted(() => ({
  convertFileSrc: vi.fn((path: string) => `asset://localhost/${encodeURIComponent(path)}`),
  inTauri: true,
}));

vi.mock("@tauri-apps/api/core", () => ({
  convertFileSrc: bridge.convertFileSrc,
  isTauri: () => bridge.inTauri,
}));

beforeEach(() => {
  bridge.convertFileSrc.mockClear();
  bridge.inTauri = true;
});

const i18n = createI18n();

/** One title as the core sends it. */
function title(overrides: Partial<LibraryTitleDto> = {}): LibraryTitleDto {
  return {
    id: 1,
    name: "Alien",
    kind: "movies",
    stub: true,
    items: 1,
    files: 2,
    bytes: 8 * 1024 * 1024 * 1024,
    missingFiles: 0,
    poster: null,
    // False by default: a fixture is not a title the backfill should ask the core about, and a test that
    // wants one says so.
    needsPoster: false,
    ...overrides,
  };
}

function library(state: Partial<LibraryTitles> = {}): LibraryTitles {
  return { titles: [], loading: false, failed: false, refresh: () => Promise.resolve(), ...state };
}

/** One parked file as the core sends it. */
function parked(overrides: Partial<UnmatchedFileDto> = {}): UnmatchedFileDto {
  return {
    id: 7,
    name: "The Matrix 2049.mkv",
    parsedTitle: "The Matrix",
    parsedYear: 2049,
    parsedSeason: null,
    parsedEpisode: null,
    reason: "`2049` could be a year or part of the title",
    suggestedTitleName: null,
    sizeBytes: 2_000_000_000,
    ...overrides,
  };
}

/** The queue as a state, with its action recorded rather than performed. */
function queue(
  state: Partial<UnmatchedQueue> = {},
): UnmatchedQueue & { readonly matched: number[] } {
  const record = { matched: [] as number[] };
  return {
    files: [],
    loading: false,
    failed: false,
    busy: false,
    error: null,
    match: (file: UnmatchedFileDto) => {
      record.matched.push(file.id);
      return Promise.resolve();
    },
    matchAs: (file: UnmatchedFileDto, titleId: number) => {
      record.matched.push(file.id + titleId);
      return Promise.resolve();
    },
    refresh: () => Promise.resolve(),
    ...state,
    ...record,
  };
}

/** One remembered rule as the core sends it. */
function rule(overrides: Partial<MatchRuleDto> = {}): MatchRuleDto {
  return { id: 3, pattern: "the matrix", titleName: "The Matrix", ...overrides };
}

/** The rules as a state, with the removal recorded rather than performed. */
function rules(state: Partial<MatchRules> = {}): MatchRules & { readonly forgotten: number[] } {
  const record = { forgotten: [] as number[] };
  return {
    rules: [],
    loading: false,
    failed: false,
    busy: false,
    error: null,
    forget: (id: number) => {
      record.forgotten.push(id);
      return Promise.resolve();
    },
    refresh: () => Promise.resolve(),
    ...state,
    ...record,
  };
}

/** The scan control as a state, with its action recorded rather than performed. */
function scanControl(state: Partial<ScanControl> = {}): ScanControl & { readonly starts: number } {
  const record = { starts: 0 };
  return {
    jobId: null,
    phase: null,
    permille: 0,
    starting: false,
    running: false,
    finished: 0,
    error: null,
    start: () => {
      record.starts += 1;
      return Promise.resolve();
    },
    ...state,
    ...record,
  };
}

/**
 * The screen as markup rather than as a mounted tree.
 *
 * The table is a projection of what the core sent and nothing else, so static rendering is exactly
 * what this screen has to get right: the read, the empty case, and the failed case are three
 * different screens and the test asserts they say three different things.
 */
function markup(
  state: LibraryTitles,
  scan: ScanControl = scanControl(),
  unmatched: UnmatchedQueue = queue(),
  remembered: MatchRules = rules(),
  view: LibraryView = "grid",
): string {
  return renderToStaticMarkup(
    <I18nextProvider i18n={i18n}>
      <LibraryScreen
        onNavigate={() => undefined}
        onView={() => undefined}
        rules={remembered}
        scan={scan}
        titles={state}
        unmatched={unmatched}
        view={view}
      />
    </I18nextProvider>,
  );
}

/**
 * The same screen drawn as the table.
 *
 * The two drawings answer different questions and only one of them has columns, so a test about a
 * *column* has to ask for the table — asserting it against the default would be asserting a column
 * that is not on screen.
 */
function markupList(state: LibraryTitles): string {
  return markup(state, scanControl(), queue(), rules(), "list");
}

describe("LibraryScreen", () => {
  it("lists every title with its kind, copies, and size", () => {
    const html = markup(library({ titles: [title()] }));

    expect(html).toContain("Alien");
    expect(html).toContain("Film");
    expect(html).toContain("8 GB");
  });

  it("says a title a provider has not described yet is a stub", () => {
    // The honest answer to "why is there no poster": the metadata road is a later slice, and a row
    // drawn like any other would promise artwork that is not coming yet.
    const stub = markup(library({ titles: [title({ stub: true })] }));
    const described = markup(library({ titles: [title({ stub: false })] }));

    expect(stub).toContain("No metadata yet");
    expect(described).not.toContain("No metadata yet");
  });

  it("shows a missing count only when something is missing", () => {
    // A column of dashes for a healthy library is noise, and a zero would read as "none missing"
    // on a row where the scan has not run yet: the dash says the question does not apply.
    const none = markupList(library({ titles: [title({ missingFiles: 0 })] }));
    const some = markupList(library({ titles: [title({ files: 3, missingFiles: 1 })] }));

    expect(none).toContain("—");
    expect(some).toContain("Missing");
    expect(some).toContain("1");
  });

  it("draws a grid of tiles by default, because that is the drawing a library is opened for", () => {
    const html = markup(library({ titles: [title()] }));

    expect(html).toContain('data-library-grid=""');
    expect(html).toContain('aria-label="Titles in your library"');
    expect(html).not.toContain("<table");
    expect(html).toContain('aria-pressed="true">Grid</button>');
    expect(html).toContain('aria-pressed="false">List</button>');
  });

  it("draws the table when the list is chosen, and the control says which drawing is on", () => {
    const html = markupList(library({ titles: [title()] }));

    expect(html).toContain("<table");
    expect(html).not.toContain('data-library-grid=""');
    // Both choices are on screen and exactly one of them is pressed, which is what makes the control
    // a choice rather than two buttons: a screen reader announces the pressed state, not the colour.
    expect(html).toContain('aria-pressed="true">List</button>');
    expect(html).toContain('aria-pressed="false">Grid</button>');
  });

  it("offers no view control over a library with nothing in it", () => {
    // A toggle over an empty library is a control for a choice nobody can see, and the empty state is
    // where a scan is wanted instead.
    const html = markup(library());

    expect(html).not.toContain('data-library-view=""');
  });

  it("says how many copies are missing on the tile, in words rather than as a dash", () => {
    // The table's dash is a mark in a column that exists to be scanned down. A tile has no column, so
    // the same fact is a phrase — and its absence on a healthy title is silence, not a zero.
    const none = markup(library({ titles: [title({ missingFiles: 0 })] }));
    const some = markup(library({ titles: [title({ files: 3, missingFiles: 1 })] }));

    expect(none).not.toContain("missing");
    expect(some).toContain("1 copy missing");
    expect(markup(library({ titles: [title({ files: 3, missingFiles: 3 })] }))).toContain(
      "3 copies missing",
    );
  });

  it("loads a tile's poster over the asset protocol, and never through IPC", () => {
    // The whole point of the poster travelling on the list read: the core resolves the cache path for
    // every row, and only `lib/media.ts` turns one into a URL (AGENTS R11).
    const html = markup(library({ titles: [title({ poster: "/cache/images/9f2c.webp" })] }));

    expect(bridge.convertFileSrc).toHaveBeenCalledWith("/cache/images/9f2c.webp");
    expect(html).toContain("asset://localhost/");

    // A reference that is not a cache path is no picture at all, which is the reserved box.
    bridge.convertFileSrc.mockClear();
    const remote = markup(
      library({ titles: [title({ poster: "https://image.tmdb.org/t/p/w342/poster.jpg" })] }),
    );

    expect(bridge.convertFileSrc).not.toHaveBeenCalled();
    expect(remote).not.toContain("asset://localhost/");
  });

  it("distinguishes an empty library from a read that failed", () => {
    const empty = markup(library());
    const failed = markup(library({ failed: true }));

    expect(empty).toContain("Nothing has been scanned in yet.");
    expect(empty).not.toContain("could not be read");
    expect(failed).toContain("could not be read");
    expect(failed).not.toContain("Nothing has been scanned in yet.");
  });

  it("draws nothing at all while the read is in flight", () => {
    // A spinner is PG-E21-12's, and an empty table would say "you have nothing" before the core has
    // answered. Drawing neither is the only honest option until the skeleton exists.
    const loading = markup(library({ loading: true }));

    expect(loading).not.toContain("Nothing has been scanned in yet.");
    expect(loading).not.toContain("could not be read");
    expect(loading).not.toContain("<table");
  });

  it("gives a series its own kind rather than calling everything a film", () => {
    const html = markup(library({ titles: [title({ kind: "shows" })] }));

    expect(html).toContain("Series");
  });

  it("offers a scan even when the library is empty", () => {
    // The empty state is exactly where a scan is wanted: the copy says to add a folder and scan,
    // and a screen that made the user walk to Settings to do it would be a worse answer than the
    // button it already has the command for.
    const html = markup(library());

    expect(html).toContain("Scan now");
    expect(html).toContain("Nothing has been scanned in yet.");
  });

  it("shows the scan's progress while one is running and refuses a second", () => {
    const running = markup(
      library(),
      scanControl({ jobId: "01J", running: true, phase: "running", permille: 420 }),
    );

    expect(running).toContain("Scanning");
    expect(running).toContain('role="progressbar"');
    expect(running).toContain(`disabled=""`);
  });

  it("says how the last scan ended rather than leaving the last word on screen", () => {
    // "Scanning…" left behind after a failed scan is a screen lying about the state of the world,
    // so each terminal phase gets its own sentence.
    const succeeded = markup(library(), scanControl({ phase: "succeeded" }));
    const failed = markup(library(), scanControl({ phase: "failed" }));

    expect(succeeded).toContain("Scan finished.");
    expect(failed).toContain("The last scan did not finish.");
    expect(succeeded).not.toContain("Scanning");
  });

  it("shows a parked file with what its name read and why it could not be placed", () => {
    // The question a person is answering is which of those values to trust, so both halves are on
    // the row: the parse's reading above, and the reason below.
    const html = markup(library(), scanControl(), queue({ files: [parked()] }));

    expect(html).toContain("The Matrix 2049.mkv");
    expect(html).toContain("The Matrix (2049)");
    expect(html).toContain("could be a year or part of the title");
    expect(html).toContain("Match");
  });

  it("says which existing title a parked name looks like, and files it there", () => {
    // The offer is what makes the queue answerable without a picker: the library already holds a
    // title this name could mean, so the question is a yes rather than a search.
    const html = markup(
      library(),
      scanControl(),
      queue({ files: [parked({ suggestedTitleName: "The Matrix" })] }),
    );

    expect(html).toContain("Looks like The Matrix");
    expect(html).toContain("File under The Matrix");
  });

  it("asks a plain match when nothing was close enough", () => {
    const html = markup(library(), scanControl(), queue({ files: [parked()] }));

    expect(html).toContain("Match");
    expect(html).not.toContain("Looks like");
  });

  it("treats a missing or empty suggestion as no suggestion at all", () => {
    // Found by looking at a capture: a payload that omits the field arrives as `undefined`, and a
    // check for `null` took the suggestion branch with nothing in it — a bare "Looks like" on screen.
    // A label with a hole in it is worse than no label.
    const missing = { ...parked() };
    delete (missing as { suggestedTitleName?: unknown }).suggestedTitleName;
    const html = markup(library(), scanControl(), queue({ files: [missing] }));

    expect(html).not.toContain("Looks like");
    expect(html).toContain("Match");
  });

  it("offers nothing to match when the name read no title", () => {
    // `1080p.mkv` reads a resolution and no title, so there is nothing to place it as: the button is
    // there and disabled, which says why rather than hiding the row.
    const html = markup(
      library(),
      scanControl(),
      queue({
        files: [
          parked({
            name: "1080p.mkv",
            parsedTitle: null,
            parsedYear: null,
            reason: "no title could be read",
          }),
        ],
      }),
    );

    expect(html).toContain("Nothing was read from this name");
    expect(html).toContain(`disabled=""`);
  });

  it("says a queue is empty only when the read answered", () => {
    const empty = markup(library(), scanControl(), queue());
    const failed = markup(library(), scanControl(), queue({ failed: true }));

    expect(empty).toContain("Nothing is waiting.");
    expect(failed).not.toContain("Nothing is waiting.");
    expect(failed).not.toContain("Needs a decision");
  });

  it("disables every match while one is in flight", () => {
    const busy = markup(library(), scanControl(), queue({ files: [parked()], busy: true }));

    expect(busy).toContain(`disabled=""`);
  });

  it("shows a remembered rule, so an answer that keeps acting can be seen", () => {
    // A rule places every later name that reads the same way, so it acts without the user present.
    // The screen's job is to make that visible: what a name reads, and where it is filed.
    const html = markup(library(), scanControl(), queue(), rules({ rules: [rule()] }));

    expect(html).toContain("Remembered");
    expect(html).toContain("the matrix");
    expect(html).toContain("The Matrix");
    expect(html).toContain("Forget");
  });

  it("says nothing about rules when there are none or the read failed", () => {
    // An empty "Remembered" section would be a heading with nothing under it, and a failed read must
    // not look like a library with no rules — the same distinction every other read on this screen
    // makes.
    const none = markup(library(), scanControl(), queue(), rules());
    const failed = markup(library(), scanControl(), queue(), rules({ failed: true }));

    expect(none).not.toContain("Remembered");
    expect(failed).not.toContain("Remembered");
  });

  it("says a rule is withdrawn as forgetting, not as deleting the file", () => {
    // The word matters: the files a rule placed stay, and a label that suggested otherwise would
    // make a user hesitate to withdraw a wrong answer.
    const html = markup(library(), scanControl(), queue(), rules({ rules: [rule()] }));

    expect(html).toContain("Forg");
    expect(html).not.toContain("Delete");
  });

  it("shows why a scan was refused, in the core's own words", () => {
    const refused = markup(
      library(),
      scanControl({
        error: {
          code: "CONFLICT",
          message: "conflict",
          detail: "A scan is already running.",
          retryable: false,
        },
      }),
    );

    expect(refused).toContain("A scan is already running.");
  });
});
