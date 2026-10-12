import { createI18n } from "@peregrine/i18n";
import type { LibraryRootDto } from "@peregrine/ipc-client";
import { renderToStaticMarkup } from "react-dom/server";
import { I18nextProvider } from "react-i18next";
import { describe, expect, it } from "vitest";
import type { LibraryRoots } from "../hooks/useLibraryRoots.js";
import { LibrariesSection } from "./LibrariesSection.js";

const i18n = createI18n();

/** One root as the core sends it. */
function root(id: number, path: string, overrides: Partial<LibraryRootDto> = {}): LibraryRootDto {
  return {
    id,
    path,
    kind: "mixed",
    scanMode: "incremental",
    lastScanAt: null,
    createdAt: 1_700_000_000_000,
    ...overrides,
  };
}

/** The read as a state, with the two operations recorded rather than performed. */
function libraries(
  state: Partial<LibraryRoots> = {},
): LibraryRoots & { readonly added: number; readonly removed: number[] } {
  const record = {
    added: 0,
    removed: [] as number[],
  };
  return {
    roots: [],
    loading: false,
    failed: false,
    busy: false,
    error: null,
    add: () => {
      record.added += 1;
      return Promise.resolve(null);
    },
    remove: (id: number) => {
      record.removed.push(id);
      return Promise.resolve(true);
    },
    ...state,
    ...record,
  };
}

/**
 * The section as markup rather than as a mounted tree.
 *
 * The list is a projection of the read and nothing else, so static rendering is what this
 * component has to get right; what a click does is the hook's, and the hook is exercised through
 * the IPC bridge rather than here.
 */
function markup(state: LibraryRoots): string {
  return renderToStaticMarkup(
    <I18nextProvider i18n={i18n}>
      <LibrariesSection libraries={state} />
    </I18nextProvider>,
  );
}

describe("LibrariesSection", () => {
  it("lists every root by its path, newest first as the core sent them", () => {
    const html = markup(
      libraries({
        roots: [root(2, "/media/Shows"), root(1, "/media/Movies")],
      }),
    );

    expect(html).toContain("/media/Shows");
    expect(html).toContain("/media/Movies");
    expect(html.indexOf("/media/Shows")).toBeLessThan(html.indexOf("/media/Movies"));
  });

  it("says whether a root has ever been scanned, without inventing a date", () => {
    const html = markup(
      libraries({
        roots: [
          root(1, "/media/Movies", { lastScanAt: 1_700_000_000_000 }),
          root(2, "/media/Shows"),
        ],
      }),
    );

    expect(html).toContain("scanned");
    expect(html).toContain("not scanned yet");
  });

  it("names the kind the core holds rather than guessing from the path", () => {
    const html = markup(libraries({ roots: [root(1, "/media/Films", { kind: "movies" })] }));

    expect(html).toContain("Films");
    expect(html).not.toContain("Films and series");
  });

  it("offers a folder to add and a removal per root", () => {
    const html = markup(libraries({ roots: [root(1, "/media/Movies")] }));

    expect(html).toContain("Add folder");
    expect(html).toContain("Remove");
  });

  it("distinguishes an empty list from a read that failed", () => {
    const empty = markup(libraries());
    const failed = markup(libraries({ failed: true }));

    expect(empty).toContain("No folders yet.");
    expect(empty).not.toContain("could not be read");
    expect(failed).toContain("could not be read");
    expect(failed).not.toContain("No folders yet.");
  });

  it("shows the refusal beside the list and disables both writes while one is in flight", () => {
    const refused = markup(
      libraries({
        roots: [root(1, "/media/Movies")],
        error: "That folder is already one of your libraries.",
      }),
    );
    const busy = markup(libraries({ roots: [root(1, "/media/Movies")], busy: true }));

    expect(refused).toContain("That folder is already one of your libraries.");
    expect(busy).toContain("disabled");
  });

  it("does not offer to add a folder when the core could not be read", () => {
    // A button that would write into a list the screen cannot see is worse than no button.
    const html = markup(libraries({ failed: true }));

    expect(html).toContain("disabled");
  });
});
