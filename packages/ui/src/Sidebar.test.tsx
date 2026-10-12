import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { Sidebar, SidebarItem } from "./Sidebar.js";

/** The sidebar's own markup, so an assertion cannot accidentally match the screen. */
function nav(markup: string): string {
  const start = markup.indexOf("<nav");
  const end = markup.indexOf("</nav>");
  expect(start).toBeGreaterThan(-1);
  expect(end).toBeGreaterThan(start);
  return markup.slice(start, end);
}

function row(label: string, current = false): string {
  return renderToStaticMarkup(
    <Sidebar label="Screens">
      <SidebarItem current={current} icon="compass" label={label} onSelect={() => undefined} />
    </Sidebar>,
  );
}

describe("Sidebar", () => {
  it("is one labelled navigation landmark", () => {
    const markup = nav(
      renderToStaticMarkup(
        <Sidebar label="Screens">
          <SidebarItem icon="compass" label="Discover" onSelect={() => undefined} />
        </Sidebar>,
      ),
    );
    expect(markup).toContain('aria-label="Screens"');
    expect(markup.match(/<nav/gu)).toHaveLength(1);
  });

  it("holds no rows when it is given none, and still names itself", () => {
    const markup = nav(renderToStaticMarkup(<Sidebar label="Screens">{null}</Sidebar>));
    expect(markup).toContain('aria-label="Screens"');
    expect(markup).not.toContain("<button");
  });

  it("announces exactly the row that is on show, and only that one", () => {
    const markup = nav(
      renderToStaticMarkup(
        <Sidebar label="Screens">
          <SidebarItem icon="compass" label="Discover" onSelect={() => undefined} />
          <SidebarItem current icon="pulse" label="Diagnostics" onSelect={() => undefined} />
        </Sidebar>,
      ),
    );
    expect(markup.match(/aria-current="page"/gu)).toHaveLength(1);
    // The marked row is the current one, not merely the first one rendered.
    expect(markup.indexOf('aria-current="page"')).toBeGreaterThan(markup.indexOf("Discover"));
  });

  it("keeps every row's label available at rail width", () => {
    // `sr-only` at the narrow width hides the text visually and keeps it in the
    // accessibility tree; `hidden` would take the row's name away with it.
    const markup = nav(row("Discover"));
    expect(markup).toContain("Discover");
    expect(markup).toContain("max-lg:sr-only");
  });

  it("clears the pointer target minimum and keeps the token focus ring", () => {
    const markup = nav(row("Discover"));
    expect(markup).toContain("h-8");
    expect(markup).toContain("focus-visible:outline-focus-ring");
    expect(markup).toContain("motion-reduce:transition-none");
  });

  it("is a button rather than a link until the router lands (PG-E7-02)", () => {
    const markup = nav(row("Discover"));
    expect(markup).toContain("<button");
    expect(markup).toContain('type="button"');
    expect(markup).not.toContain("<a ");
  });
});
