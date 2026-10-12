import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { Panel } from "./Panel.js";

describe("Panel", () => {
  it("puts content on the design system's surface with its border", () => {
    const markup = renderToStaticMarkup(
      <Panel>
        <p>content</p>
      </Panel>,
    );
    expect(markup).toContain("bg-surface-1");
    expect(markup).toContain("border-border-subtle");
    expect(markup).toContain("rounded-md");
    expect(markup).toContain("content");
  });

  it("floats nothing, so it carries no elevation", () => {
    // §4 allows elevation only for what sits above the page — a menu, a popover, a dialog.
    const markup = renderToStaticMarkup(
      <Panel>
        <p>content</p>
      </Panel>,
    );
    expect(markup).not.toContain("shadow");
  });

  it("lets the caller decide its interior", () => {
    // A group of rows wants dividers and no padding; a log viewport wants padding and a
    // scroll. The surface is the same, so only the surface is here.
    const markup = renderToStaticMarkup(
      <Panel className="overflow-auto p-3">
        <pre>log</pre>
      </Panel>,
    );
    expect(markup).toContain("overflow-auto");
    expect(markup).toContain("p-3");
  });
});
