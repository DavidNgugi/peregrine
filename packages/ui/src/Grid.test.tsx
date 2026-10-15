import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { Grid, gridColumns, gridEndReached, gridWindow } from "./Grid.js";
import { TitleCard } from "./TitleCard.js";

const TILE = 144;
const ROW = 228;

function windowOf(
  overrides: Partial<Parameters<typeof gridWindow>[0]> = {},
): ReturnType<typeof gridWindow> {
  return gridWindow({
    columns: 4,
    itemCount: 1000,
    overscanRows: 1,
    rowHeight: ROW,
    scrollTop: 0,
    viewportHeight: 800,
    ...overrides,
  });
}

describe("gridColumns", () => {
  it("fits as many tiles as the gaps allow", () => {
    // 4 tiles of 144 and 3 gaps of 12 is 612, so 612 fits four and 611 fits three.
    expect(gridColumns(612, TILE, 12)).toBe(4);
    expect(gridColumns(611, TILE, 12)).toBe(3);
    expect(gridColumns(624, TILE, 12)).toBe(4);
  });

  it("never answers zero columns, however narrow the container", () => {
    // A grid of no columns draws nothing at all, which reads as a failed load rather than as a window
    // dragged narrower than a poster.
    expect(gridColumns(1, TILE, 12)).toBe(1);
    expect(gridColumns(0, TILE, 12)).toBe(1);
    expect(gridColumns(-20, TILE, 12)).toBe(1);
  });

  it("survives a configuration that cannot be laid out", () => {
    expect(gridColumns(800, 0, 12)).toBe(1);
    expect(gridColumns(800, TILE, 0)).toBe(5);
  });
});

describe("gridWindow", () => {
  it("draws nothing when there is nothing to draw", () => {
    expect(windowOf({ itemCount: 0 })).toEqual({ first: 0, last: 0, offset: 0, rows: 0 });
    expect(windowOf({ columns: 0 })).toEqual({ first: 0, last: 0, offset: 0, rows: 0 });
    expect(windowOf({ rowHeight: 0 })).toEqual({ first: 0, last: 0, offset: 0, rows: 0 });
  });

  it("draws the first rows before the container has been measured", () => {
    // The state every first paint is in: `clientHeight` is 0 until layout, and an empty window there
    // would show a blank grid until the observer's first callback.
    const window_ = windowOf({ viewportHeight: 0 });
    expect(window_.first).toBe(0);
    expect(window_.rows).toBe(2);
  });

  it("covers the viewport and one row of overscan on each side", () => {
    // A viewport of 800 with 228 px rows shows rows 2..5 from an offset of 456, so with one row of
    // overscan the window is rows 1..6 — clipped at the top when there is nothing above row 0.
    const atTop = windowOf();
    expect(atTop.offset).toBe(0);
    expect(atTop.last).toBe(20);

    const mid = windowOf({ scrollTop: 2 * ROW });
    expect(mid.first).toBe(4);
    expect(mid.last).toBe(28);
    expect(mid.offset).toBe(ROW);
  });

  it("stops at the last item rather than over-rendering past the end", () => {
    const atEnd = windowOf({ itemCount: 40, scrollTop: 9 * ROW });
    expect(atEnd.last).toBe(40);
    expect(atEnd.first).toBeLessThan(40);
    expect(atEnd.rows).toBeLessThanOrEqual(Math.ceil(800 / ROW) + 1 + 1);
  });

  it("clamps an offset past the end, which is what a shrinking list leaves behind", () => {
    // A filter that cuts 1 000 results to 40 while the grid is scrolled near its bottom: the offset is
    // still 9 000 and every row after the fortieth has gone.
    const shrunk = windowOf({ itemCount: 40, scrollTop: 900 * ROW });
    expect(shrunk.last).toBe(40);
    expect(shrunk.first).toBe(36);
    expect(shrunk.rows).toBe(1);
    expect(shrunk.offset).toBe(9 * ROW);
  });

  it("treats a rubber-band overscroll as the top", () => {
    const pulled = windowOf({ scrollTop: -120 });
    expect(pulled.first).toBe(0);
    expect(pulled.offset).toBe(0);
  });

  it("keeps the drawn slice bounded at five thousand items", () => {
    // The acceptance in one assertion: what the DOM holds depends on the viewport, not on the
    // catalogue. Four columns of 228 px rows in an 800 px viewport is 24 tiles, overscan included.
    const huge = windowOf({ itemCount: 5000, scrollTop: 200 * ROW });
    expect(huge.last - huge.first).toBeLessThanOrEqual(24);
  });

  it("handles a grid narrower than the item count in its last row", () => {
    // One item in a four-column grid is one row, and the last row's three missing cells must not be
    // drawn as empty tiles or counted as rows.
    const one = windowOf({ itemCount: 1, viewportHeight: 2000 });
    expect(one.first).toBe(0);
    expect(one.last).toBe(1);
    expect(one.rows).toBe(1);

    const five = windowOf({ itemCount: 5, viewportHeight: 2000 });
    expect(five.last).toBe(5);
    expect(five.rows).toBe(2);
  });
});

describe("Grid", () => {
  function grid(
    itemCount: number,
    overrides: Partial<Parameters<typeof Grid<number>>[0]> = {},
  ): string {
    const items = Array.from({ length: itemCount }, (_, index) => index);
    return renderToStaticMarkup(
      <Grid
        columns={4}
        gap={12}
        itemKey={(item) => item}
        items={items}
        label="Results"
        renderItem={(item) => (
          <TitleCard href={`/title/movie/${String(item)}`} title={`#${String(item)}`} />
        )}
        rowHeight={ROW}
        tileWidth={TILE}
        {...overrides}
      />,
    );
  }

  it("names the region and gives it one tab stop", () => {
    const markup = grid(50);
    expect(markup).toContain('aria-label="Results"');
    expect(markup).toContain('role="list"');
    expect(markup.match(/tabindex="0"/gu)).toHaveLength(1);
  });

  it("renders a bounded number of tiles for a catalogue of five thousand", () => {
    const markup = grid(5000);
    const tiles = markup.match(/role="listitem"/gu)?.length ?? 0;
    expect(tiles).toBeGreaterThan(0);
    expect(tiles).toBeLessThanOrEqual(24);
  });

  it("carries the full height in one spacer, so the scrollbar is honest", () => {
    // 5 000 items in four columns is 1 250 rows of 228 px.
    expect(grid(5000)).toContain(`height:${String(1250 * ROW)}px`);
  });

  it("draws nothing but the region when it has no items", () => {
    const markup = grid(0);
    expect(markup).toContain('aria-label="Results"');
    expect(markup).not.toContain("listitem");
    expect(markup).toContain("height:0px");
  });

  it("keys each tile by the caller's domain id", () => {
    expect(grid(6)).toContain('href="/title/movie/2"');
  });
  it("reserves placeholder rows in the flow while a page is in flight", () => {
    // The affordance a reader needs at the end of a loaded page: without it, "the next page is on
    // its way" and "there is nothing more" look exactly alike. The placeholders are laid out where
    // the arriving tiles will be, so the geometry does not move when they land.
    const markup = grid(4, {
      loading: true,
      placeholder: <span data-testid="placeholder" />,
      placeholderRows: 1,
    });

    // Four items in four columns is one row; one reserved row of four cells is four placeholders.
    const placeholders = markup.match(/data-testid="placeholder"/gu) ?? [];
    expect(placeholders).toHaveLength(4);
    // A skeleton is decorative: it is hidden from assistive technology, and the *fact* that
    // something is loading is said in words by the screen (`role="status"`), not by the shape.
    expect(markup).toContain('aria-hidden="true"');
    // And the real tiles are still drawn beside them.
    expect(markup).toContain("/title/movie/0");
  });

  it("reserves nothing when no page is in flight", () => {
    // The placeholder is inert without `loading`: a grid that drew placeholders on its own would
    // tell a reader a page was coming when none had been asked for.
    const markup = grid(4, { placeholder: <span data-testid="placeholder" /> });

    expect(markup).not.toContain('data-testid="placeholder"');
  });

  it("carries the reserved rows in the spacer, so the scrollbar is honest while a page is in the air", () => {
    // The scrollbar has to account for the page on its way, or a reader scrolls to a bottom that
    // moves as the tiles arrive. Four items is one row of four columns; two reserved rows make three.
    expect(grid(4)).toContain(`height:${String(ROW)}px`);
    expect(grid(4, { loading: true, placeholderRows: 2 })).toContain(`height:${String(3 * ROW)}px`);
  });
});

describe("gridEndReached", () => {
  it("never fires for an empty list, whatever the threshold", () => {
    // The boundary the browse screen reaches on a filter that matches nothing: there is no page to
    // ask for, and a "load more" fired there would ask a provider for one for ever.
    expect(gridEndReached(0, windowOf({ itemCount: 0 }), 8)).toBe(false);
    expect(gridEndReached(0, windowOf({ itemCount: 0 }), 0)).toBe(false);
  });

  it("fires when the drawn slice reaches the end", () => {
    // One row of four visible with one of overscan draws eight of eight, which is the end.
    const atEnd = windowOf({ itemCount: 8, scrollTop: ROW });
    expect(atEnd.last).toBe(8);
    expect(gridEndReached(8, atEnd, 0)).toBe(true);
    expect(gridEndReached(8, atEnd, 8)).toBe(true);
  });

  it("fires within the threshold and not before it", () => {
    // A hundred items in four columns at the top draws five rows — the four the viewport holds and
    // one of overscan — so the slice ends at item 20. The end is then twenty items away, which a
    // threshold of eight must not treat as arrived; a list of twenty-eight is exactly eight away and
    // one of twenty-nine is not.
    const top = windowOf({ itemCount: 100, scrollTop: 0 });
    expect(top.last).toBe(20);
    expect(gridEndReached(20, top, 0)).toBe(true);
    expect(gridEndReached(28, top, 8)).toBe(true);
    expect(gridEndReached(29, top, 8)).toBe(false);
    expect(gridEndReached(100, top, 8)).toBe(false);
  });

  it("treats a negative threshold as no threshold", () => {
    // A caller that computed a negative reach — a page size minus something larger than it — means
    // "only at the end", not "always".
    const top = windowOf({ itemCount: 100, scrollTop: 0 });
    expect(gridEndReached(20, top, -5)).toBe(true);
    expect(gridEndReached(21, top, -5)).toBe(false);
  });
});
