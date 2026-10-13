import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { Grid, gridColumns, gridWindow } from "./Grid.js";
import { TitleCard } from "./TitleCard.js";

const TILE = 144;
const ROW = 228;

function windowOf(overrides: Partial<Parameters<typeof gridWindow>[0]> = {}): ReturnType<
  typeof gridWindow
> {
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
  function grid(itemCount: number, overrides: Partial<Parameters<typeof Grid<number>>[0]> = {}): string {
    const items = Array.from({ length: itemCount }, (_, index) => index);
    return renderToStaticMarkup(
      <Grid
        columns={4}
        gap={12}
        itemKey={(item) => item}
        items={items}
        label="Results"
        renderItem={(item) => <TitleCard href={`/title/movie/${String(item)}`} title={`#${String(item)}`} />}
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
});
