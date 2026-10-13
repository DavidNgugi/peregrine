import { createI18n } from "@peregrine/i18n";
import { renderToStaticMarkup } from "react-dom/server";
import { I18nextProvider } from "react-i18next";
import { describe, expect, it } from "vitest";
import { DiscoverScreen } from "./DiscoverScreen.js";

const i18n = createI18n();

function markup(): string {
  return renderToStaticMarkup(
    <I18nextProvider i18n={i18n}>
      <DiscoverScreen onNavigate={() => undefined} />
    </I18nextProvider>,
  );
}

describe("DiscoverScreen", () => {
  it("offers a search box, since a search is what the screen does", () => {
    // A submitted search rather than a typed one: it leaves the machine, and a request per keystroke is
    // a request per keystroke against someone else's API.
    const html = markup();
    expect(html).toContain("Search for a title");
    expect(html).toContain("<form");
  }, 10_000);

  it("names itself once, with a first-level heading", () => {
    const html = markup();
    expect(html.match(/<h1/gu)).toHaveLength(1);
    expect(html).toContain("Discover");
  });

  it("describes the state it is in rather than drawing nothing", () => {
    const html = markup();
    expect(html).toContain("Nothing here yet");
    expect(html).toContain("Peregrine has no catalogue data yet.");
  });

  it("is not a spinner (§11 rule 2)", () => {
    const html = markup();
    expect(html).not.toContain("animate-spin");
    expect(html).not.toContain('role="progressbar"');
  });

  it("carries the screen, so its empty state uses a section heading", () => {
    // The whole window holds this and nothing else, so the copy is not caption-sized.
    expect(markup()).toContain("text-title-2");
  });
});
