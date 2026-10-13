import { createI18n } from "@peregrine/i18n";
import { renderToStaticMarkup } from "react-dom/server";
import { I18nextProvider } from "react-i18next";
import { describe, expect, it, vi } from "vitest";
import { TitlePicker } from "./TitlePicker.js";

const i18n = createI18n();

/**
 * The picker as a static string.
 *
 * Its search is a hook, so what renders here is the *idle* state — the one a user sees the moment the
 * dialog opens. That is the state worth pinning: a picker that said "no results" before anything was
 * typed would be answering a question nobody asked, and the two are one block for exactly that
 * reason.
 */
function markup(): string {
  return renderToStaticMarkup(
    <I18nextProvider i18n={i18n}>
      <TitlePicker fileName="The Matrix 2049.mkv" onChoose={vi.fn()} onClose={vi.fn()} />
    </I18nextProvider>,
  );
}

describe("TitlePicker", () => {
  it("names the file it is placing and offers a search", () => {
    const html = markup();

    expect(html).toContain("File this under");
    expect(html).toContain("The Matrix 2049.mkv");
    expect(html).toContain("Search your catalogue");
  });

  it("says there is nothing to choose from before anything is typed", () => {
    const html = markup();

    expect(html).toContain("Nothing to choose from yet.");
    expect(html).not.toContain("did not run");
  });

  it("is a dialog, so the page behind it is not reachable", () => {
    // The confirmation rule from AGENTS R22 and the reason a dialog exists: this is one decision, and
    // the rest of the screen waits for it.
    const html = markup();

    expect(html).toContain('role="dialog"');
    expect(html).toContain('aria-modal="true"');
    expect(html).toContain("aria-labelledby=");
  });
});
