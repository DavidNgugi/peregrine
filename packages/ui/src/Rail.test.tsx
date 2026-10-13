import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { nextRailIndex, Rail, showsChevrons } from "./Rail.js";
import { TitleCard } from "./TitleCard.js";

const ITEMS = ["a", "b", "c"] as const;

function rail(overrides: Partial<Parameters<typeof Rail<string>>[0]> = {}): string {
  return renderToStaticMarkup(
    <Rail
      itemKey={(item) => item}
      items={ITEMS}
      renderItem={(item, tabIndex) => (
        <TitleCard href={`/title/movie/${item}`} tabIndex={tabIndex} title={item} />
      )}
      title="Trending"
      {...overrides}
    />,
  );
}

describe("nextRailIndex", () => {
  it("steps one item in each direction", () => {
    expect(nextRailIndex(0, "ArrowRight", 3)).toBe(1);
    expect(nextRailIndex(2, "ArrowLeft", 3)).toBe(1);
  });

  it("stops at the ends rather than wrapping", () => {
    // A rail that wrapped would move focus from the last tile to the first, which reads as a jump to
    // somewhere else on the screen rather than as reaching the end of a row.
    expect(nextRailIndex(2, "ArrowRight", 3)).toBe(2);
    expect(nextRailIndex(0, "ArrowLeft", 3)).toBe(0);
  });

  it("jumps to either end on Home and End", () => {
    expect(nextRailIndex(1, "Home", 3)).toBe(0);
    expect(nextRailIndex(1, "End", 3)).toBe(2);
  });

  it("leaves a key it does not own to whoever is above it", () => {
    // Escape belongs to the overlay stack and Tab to the platform: answering with the current index
    // would let the rail preventDefault a key that was never its own.
    for (const key of ["Escape", "Tab", "ArrowUp", "ArrowDown", "PageDown", "a", "Enter", " "]) {
      expect(nextRailIndex(1, key, 3), key).toBeNull();
    }
  });

  it("answers nothing for an empty rail", () => {
    expect(nextRailIndex(0, "ArrowRight", 0)).toBeNull();
    expect(nextRailIndex(0, "Home", 0)).toBeNull();
  });

  it("brings an out-of-range position back to something reachable", () => {
    // The case is a filter change under a held index: the focus was on tile nine and the rail now has
    // three, so the next arrow must land on a tile that exists.
    expect(nextRailIndex(9, "ArrowLeft", 3)).toBe(1);
    expect(nextRailIndex(9, "ArrowRight", 3)).toBe(2);
    expect(nextRailIndex(-4, "ArrowRight", 3)).toBe(1);
  });

  it("treats a single-item rail as its own both ends", () => {
    expect(nextRailIndex(0, "ArrowRight", 1)).toBe(0);
    expect(nextRailIndex(0, "ArrowLeft", 1)).toBe(0);
    expect(nextRailIndex(0, "End", 1)).toBe(0);
  });
});

describe("showsChevrons", () => {
  it("shows nothing when the rail does not overflow", () => {
    expect(showsChevrons(0, 800, 800)).toEqual({ start: false, end: false });
    expect(showsChevrons(0, 800, 801)).toEqual({ start: false, end: false });
  });

  it("shows only the forward edge at the start and only the back edge at the end", () => {
    expect(showsChevrons(0, 800, 2400)).toEqual({ start: false, end: true });
    expect(showsChevrons(1600, 800, 2400)).toEqual({ start: true, end: false });
    expect(showsChevrons(800, 800, 2400)).toEqual({ start: true, end: true });
  });

  it("does not light an edge over a sub-pixel residue", () => {
    // Measured rather than imagined: at 2× a rail scrolled fully to its end reports a third of a pixel
    // of remaining travel, and a strict comparison leaves the forward chevron lit on an immovable end.
    expect(showsChevrons(1599.67, 800, 2400)).toEqual({ start: true, end: false });
    expect(showsChevrons(0.33, 800, 2400)).toEqual({ start: false, end: true });
  });
});

describe("Rail", () => {
  it("names its region and gives it one heading", () => {
    const markup = rail();
    expect(markup).toContain("<section");
    expect(markup).toContain("Trending");
    expect(markup.match(/<h2/gu)).toHaveLength(1);
  });

  it("draws one tab stop, on the first tile, with the rest reachable by arrow", () => {
    const tabs = [...rail().matchAll(/tabindex="(-?\d+)"/gu)].map((match) => match[1]);
    expect(tabs).toEqual(["0", "-1", "-1"]);
  });

  it("keeps its heading when it has nothing to show, and draws what the caller gave it", () => {
    // A rail that vanished would take its heading with it, and a screen would jump as a rail filled.
    const bare = rail({ items: [] });
    expect(bare).toContain("Trending");
    expect(bare).not.toContain("<ul");

    expect(rail({ empty: <p>Nothing here yet</p>, items: [] })).toContain("Nothing here yet");
    expect(rail({ empty: <p>Nothing here yet</p>, items: [] })).not.toContain("<ul");
  });

  it("keeps its own box narrow enough for the edge affordances to be on screen", () => {
    // A flex child defaults to `min-width: auto`, so without `min-w-0` this wrapper grows to the
    // scroller's content width: the chevrons anchor to the far right of the overflow, off-screen, and
    // the page clips where the rail should scroll. Found by looking at the running app; the class is
    // the contract, because no static render can measure a box.
    expect(rail()).toContain('class="group/rail relative min-w-0"');
  });

  it("keeps a stable key from the data, never the position", () => {
    // The caller's `itemKey` is what React reuses tiles by; an index key would reuse a tile for a
    // different title after a filter change, which is the bug the kit's own rules name.
    expect(rail()).toContain('href="/title/movie/a"');
    expect(rail().indexOf('href="/title/movie/b"')).toBeLessThan(
      rail().indexOf('href="/title/movie/c"'),
    );
  });
});
