import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { EmptyState } from "./EmptyState.js";

describe("EmptyState", () => {
  it("says what is not here and, when there is a reason, why", () => {
    const markup = renderToStaticMarkup(
      <EmptyState body="Download and update events will appear here." title="Nothing yet." />,
    );
    expect(markup).toContain("Nothing yet.");
    expect(markup).toContain("Download and update events will appear here.");
  });

  it("is one line where the design calls for one (DESIGN_SYSTEM §13.1)", () => {
    const markup = renderToStaticMarkup(<EmptyState title="Nothing yet." />);
    expect(markup.match(/<p/gu)).toHaveLength(1);
  });

  it("draws no spinner and no illustration", () => {
    // §11 rule 2 bans a full-page spinner, and the branding that would draw an
    // illustration is PG-E22-01's; an empty state that waits for artwork never ships.
    const markup = renderToStaticMarkup(<EmptyState title="Nothing yet." />);
    expect(markup).not.toContain("<svg");
    expect(markup).not.toContain("animate-");
  });

  it("leaves its own placement to the container", () => {
    // The same block is centred in a page and inline in a settings group, so it must not
    // carry a width or a margin of its own.
    const markup = renderToStaticMarkup(<EmptyState title="Nothing yet." />);
    expect(markup).not.toContain("mx-auto");
    expect(markup).not.toContain("max-w-");
  });

  it("lets the call site place it", () => {
    const markup = renderToStaticMarkup(
      <EmptyState className="items-center" title="Nothing yet." />,
    );
    expect(markup).toContain("items-center");
  });
});
