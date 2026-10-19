import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import type { Tile } from "./api.js";
import { TileCard } from "./Home.js";

function tile(overrides: Partial<Tile> = {}): Tile {
  return {
    id: 1,
    name: "I, Robot",
    kind: "movie",
    year: 2004,
    poster: null,
    backdrop: null,
    rating: 7,
    overview: null,
    playable: false,
    percent: null,
    positionMs: 0,
    watchlisted: false,
    ...overrides,
  };
}

describe("TileCard", () => {
  it("draws the bookmark for a saved title", () => {
    expect(
      renderToStaticMarkup(<TileCard onOpen={() => {}} tile={tile({ watchlisted: true })} />),
    ).toContain("card-bookmark");
    expect(renderToStaticMarkup(<TileCard onOpen={() => {}} tile={tile()} />)).not.toContain(
      "card-bookmark",
    );
  });

  it("draws the score with a star rather than as a bare number", () => {
    const html = renderToStaticMarkup(<TileCard onOpen={() => {}} tile={tile()} />);
    expect(html).toContain("card-rating");
    expect(html).toContain("<svg");
    expect(html).toContain("7.0");
  });
});
