import { createI18n } from "@peregrine/i18n";
import type { LibraryTitleDto } from "@peregrine/ipc-client";
import { renderToStaticMarkup } from "react-dom/server";
import { I18nextProvider } from "react-i18next";
import { describe, expect, it } from "vitest";
import type { LibraryTitles } from "../hooks/useLibraryTitles.js";
import type { ScanControl } from "../hooks/useScan.js";
import { LibraryScreen } from "./LibraryScreen.js";

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
    ...overrides,
  };
}

function library(state: Partial<LibraryTitles> = {}): LibraryTitles {
  return { titles: [], loading: false, failed: false, refresh: () => Promise.resolve(), ...state };
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
function markup(state: LibraryTitles, scan: ScanControl = scanControl()): string {
  return renderToStaticMarkup(
    <I18nextProvider i18n={i18n}>
      <LibraryScreen scan={scan} titles={state} />
    </I18nextProvider>,
  );
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
    const none = markup(library({ titles: [title({ missingFiles: 0 })] }));
    const some = markup(library({ titles: [title({ files: 3, missingFiles: 1 })] }));

    expect(none).toContain("—");
    expect(some).toContain("Missing");
    expect(some).toContain("1");
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
