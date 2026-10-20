import { describe, expect, it } from "vitest";

import { panelPlacement } from "./notification-placement.js";

describe("panelPlacement", () => {
  it("keeps a narrow window's panel inside the right edge", () => {
    const placement = panelPlacement({ right: 220, bottom: 64 }, 438, 838);
    expect(placement.left).toBe(110);
    expect(placement.width).toBe(320);
    expect(placement.left + placement.width).toBeLessThanOrEqual(438 - 8);
  });

  it("right-aligns the panel when the bell has room", () => {
    const placement = panelPlacement({ right: 1000, bottom: 64 }, 1200, 900);
    expect(placement.left).toBe(1000 - 320);
    expect(placement.top).toBe(72);
  });

  it("shrinks rather than overflowing a very narrow window", () => {
    const placement = panelPlacement({ right: 200, bottom: 60 }, 240, 400);
    expect(placement.width).toBe(224);
    expect(placement.left).toBe(8);
  });
});
