import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { Screen } from "./Screen.js";

function headingId(markup: string): string {
  return /<h1[^>]*id="([^"]*)"/u.exec(markup)?.[1] ?? "";
}

describe("Screen", () => {
  it("names its region from the heading it drew rather than from a caller's string", () => {
    const markup = renderToStaticMarkup(
      <Screen title="Diagnostics">
        <p>content</p>
      </Screen>,
    );
    const id = headingId(markup);
    expect(id).not.toBe("");
    expect(markup).toContain(`aria-labelledby="${id}"`);
  });

  it("has exactly one first-level heading, which is the screen's name", () => {
    const markup = renderToStaticMarkup(
      <Screen title="Diagnostics">
        <p>content</p>
      </Screen>,
    );
    expect(markup.match(/<h1/gu)).toHaveLength(1);
    expect(markup).toContain("Diagnostics");
  });

  it("carries a subtitle when it has one and nothing when it does not", () => {
    const withSubtitle = renderToStaticMarkup(
      <Screen subtitle="What this screen is." title="Diagnostics">
        <p />
      </Screen>,
    );
    expect(withSubtitle).toContain("What this screen is.");

    const without = renderToStaticMarkup(
      <Screen title="Diagnostics">
        <p />
      </Screen>,
    );
    expect(without).not.toContain("What this screen is.");
  });

  it("scrolls inside itself, because the document never scrolls", () => {
    // `styles.css` keeps the document from scrolling so the chrome, the sidebar, and the
    // player bar cannot move; a screen longer than the window therefore has to scroll here.
    const markup = renderToStaticMarkup(
      <Screen title="Diagnostics">
        <p />
      </Screen>,
    );
    expect(markup).toContain("overflow-y-auto");
    expect(markup).toContain("h-full");
  });

  it("gives two screens different region names", () => {
    const markup = renderToStaticMarkup(
      <>
        <Screen title="One">
          <p />
        </Screen>
        <Screen title="Two">
          <p />
        </Screen>
      </>,
    );
    const ids = [...markup.matchAll(/<h1[^>]*id="([^"]*)"/gu)].map((match) => match[1]);
    expect(ids).toHaveLength(2);
    expect(ids[0]).not.toBe(ids[1]);
  });
});
