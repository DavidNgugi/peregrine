import { createI18n } from "@peregrine/i18n";
import type { LibraryTitleDto } from "@peregrine/ipc-client";
import { renderToStaticMarkup } from "react-dom/server";
import { I18nextProvider } from "react-i18next";
import { describe, expect, it } from "vitest";
import type { LibraryTitles } from "../hooks/useLibraryTitles.js";
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
  return { titles: [], loading: false, failed: false, ...state };
}

/**
 * The screen as markup rather than as a mounted tree.
 *
 * The table is a projection of what the core sent and nothing else, so static rendering is exactly
 * what this screen has to get right: the read, the empty case, and the failed case are three
 * different screens and the test asserts they say three different things.
 */
function markup(state: LibraryTitles): string {
  return renderToStaticMarkup(
    <I18nextProvider i18n={i18n}>
      <LibraryScreen titles={state} />
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
});
