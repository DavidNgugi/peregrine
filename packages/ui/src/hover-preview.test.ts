import { describe, expect, it } from "vitest";
import {
  badgeVisible,
  DWELL_MS,
  ENTER_MOTION,
  EXIT_MOTION,
  FOCUS_RING_OUTSET_PX,
  IDLE_PREVIEW,
  MIN_VISIBLE_RATIO,
  RAMP_BLUR_PX,
  RAMP_OFFSET_Y_PX,
  REVEAL_LIFT_Z,
  REVEAL_SCALE,
  REVEAL_SCALE_CAP,
  nextPreview,
  panelVisible,
  previewLift,
  previewMotion,
  previewScale,
  revealRoom,
  revealScale,
  type PreviewEvent,
  type PreviewState,
} from "./hover-preview.js";

/** The state after a list of events, from rest. */
function after(...events: readonly PreviewEvent[]): PreviewState {
  return events.reduce<PreviewState>(nextPreview, IDLE_PREVIEW);
}

const enter: PreviewEvent = { type: "pointerEnter" };
const dwell: PreviewEvent = { type: "dwellElapsed" };

describe("the dwell", () => {
  it("is 450 ms, the number §5 names", () => {
    // The card's timer reads this constant, so the test that pins it is the only place the number
    // appears twice — and a change to it has to be a change here.
    expect(DWELL_MS).toBe(450);
  });

  it("starts on pointer enter and does not reveal anything yet", () => {
    const dwelling = after(enter);

    expect(dwelling.phase).toBe("dwelling");
    expect(previewScale(dwelling)).toBe(1);
    expect(panelVisible(dwelling, false)).toBe(false);
  });

  it("reveals only when the delay has elapsed", () => {
    // The heart of the ladder: a pointer crossing a tile must produce nothing, and the transition
    // from `dwelling` to `revealed` is the timer's alone.
    expect(after(dwell).phase).toBe("idle");
    expect(after({ type: "settled" }).phase).toBe("idle");
    expect(after({ type: "scroll" }).phase).toBe("idle");
    expect(after(enter, dwell).phase).toBe("revealed");
  });

  it("fires once, however many times the timer arrives", () => {
    const revealed = after(enter, dwell);
    const again = after(enter, dwell, dwell);

    expect(again).toEqual(revealed);
    expect(previewScale(again)).toBe(REVEAL_SCALE);
  });

  it("does not restart what is already revealed", () => {
    // A pointer that re-enters without leaving — or a second enter from a nested element — must not
    // take the preview down and put it back.
    expect(after(enter, dwell, enter)).toEqual(after(enter, dwell));
  });
});

describe("what cancels the dwell", () => {
  it("cancels on pointer leave", () => {
    expect(after(enter, { type: "pointerLeave" }).phase).toBe("idle");
  });

  it("cancels on scroll", () => {
    // The card moved under a pointer that never did, so the pointer is no longer over it.
    expect(after(enter, { type: "scroll" }).phase).toBe("idle");
  });

  it("cancels when the document goes hidden", () => {
    expect(after(enter, { type: "visibilityChange", hidden: true }).phase).toBe("idle");
  });

  it("keeps the dwell when the document becomes visible again", () => {
    // `visibilitychange` fires on the way back too; that is not a reason to cancel a dwell that is
    // still running.
    expect(after(enter, { type: "visibilityChange", hidden: false }).phase).toBe("dwelling");
  });

  it("cancels below 60% visibility and keeps it at exactly 60%", () => {
    // §5 says "dropping below 60%", so the floor itself is still visible enough — the boundary a
    // comparison written `<=` would get wrong.
    expect(MIN_VISIBLE_RATIO).toBe(0.6);
    expect(after(enter, { type: "intersection", ratio: 0.59 }).phase).toBe("idle");
    expect(after(enter, { type: "intersection", ratio: 0.6 }).phase).toBe("dwelling");
    expect(after(enter, { type: "intersection", ratio: 1 }).phase).toBe("dwelling");
  });

  it("takes a revealed card back down from any of the four", () => {
    for (const event of [
      { type: "pointerLeave" },
      { type: "scroll" },
      { type: "visibilityChange", hidden: true },
      { type: "intersection", ratio: 0 },
    ] as const) {
      const left = after(enter, dwell, event);

      expect(left.phase, event.type).toBe("idle");
      expect(previewScale(left), event.type).toBe(1);
      expect(panelVisible(left, false), event.type).toBe(false);
    }
  });
});

describe("will-change, and when it is not applied", () => {
  it("is on for a card that is animating and off for one that is not", () => {
    expect(after(enter).animating).toBe(true);
    expect(after(enter, dwell).animating).toBe(true);
    expect(after(enter, { type: "settled" }).animating).toBe(false);
  });

  it("comes off after the reveal's own transition ends", () => {
    // §5 rule 3: removed on `transitionend`, never left on a grid of tiles.
    const settled = after(enter, dwell, { type: "settled" });

    expect(settled.phase).toBe("revealed");
    expect(settled.animating).toBe(false);
  });

  it("stays on through the exit a revealed card starts, and comes off when it settles", () => {
    const leaving = after(enter, dwell, { type: "pointerLeave" });

    expect(leaving.phase).toBe("idle");
    expect(leaving.animating).toBe(true);
    expect(after(enter, dwell, { type: "pointerLeave" }, { type: "settled" }).animating).toBe(
      false,
    );
  });

  it("never turns on for a dwell that was cancelled before it fired", () => {
    // Nothing transitioned, so nothing would ever fire `transitionend` to turn it off again.
    expect(after(enter, { type: "scroll" }).animating).toBe(false);
    expect(after(enter, { type: "pointerLeave" }).animating).toBe(false);
  });
});

describe("the panel", () => {
  it("is drawn only once the interaction has fired", () => {
    expect(panelVisible(IDLE_PREVIEW, false)).toBe(false);
    expect(panelVisible(after(enter), false)).toBe(false);
    expect(panelVisible(after(enter, dwell), false)).toBe(true);
  });

  it("is drawn on focus, which never scales the card", () => {
    // §5's keyboard-parity row: focus reveals the metadata panel only, and the preview is
    // pointer-only.
    expect(panelVisible(IDLE_PREVIEW, true)).toBe(true);
    expect(panelVisible(after(enter), true)).toBe(true);
    expect(previewScale(IDLE_PREVIEW)).toBe(1);
    expect(previewScale(after(enter))).toBe(1);
  });

  it("takes the tile's own badge off while it is up, focus included", () => {
    // Two scores on one tile at two contrasts is the defect this prevents; the resting tile keeps
    // its badge, which is the state every screenshot of a grid is taken in.
    expect(badgeVisible(IDLE_PREVIEW, false)).toBe(true);
    expect(badgeVisible(after(enter), false)).toBe(true);
    expect(badgeVisible(after(enter, dwell), false)).toBe(false);
    expect(badgeVisible(IDLE_PREVIEW, true)).toBe(false);
    expect(badgeVisible(after(enter, dwell, { type: "pointerLeave" }), false)).toBe(true);
  });
});
describe("the two transitions §5 names", () => {
  it("enters over --dur-3 decelerating and exits over 120 ms accelerating", () => {
    // The reveal's own values only exist in the revealed state, which a static render cannot reach —
    // so they are asserted here, against the state, rather than in the card's markup test.
    expect(previewMotion(after(enter, dwell))).toEqual(ENTER_MOTION);
    expect(previewMotion(after(enter, dwell)).duration).toBe("var(--dur-3, 180ms)");
    expect(previewMotion(after(enter, dwell)).easing).toContain("cubic-bezier(0, 0, 0, 1)");

    expect(previewMotion(IDLE_PREVIEW)).toEqual(EXIT_MOTION);
    expect(previewMotion(after(enter))).toEqual(EXIT_MOTION);
    expect(previewMotion(after(enter, dwell, { type: "pointerLeave" }))).toEqual(EXIT_MOTION);
    expect(previewMotion(IDLE_PREVIEW).duration).toBe("var(--dur-2, 120ms)");
    expect(previewMotion(IDLE_PREVIEW).easing).toContain("cubic-bezier(0.3, 0, 1, 1)");
  });

  it("names the tokens PG-E21-07 will land, with §5's numbers until then", () => {
    // A fallback rather than a second vocabulary: the day the `--dur-*` family exists, nothing here
    // changes and the reduced-motion collapse comes with it.
    for (const motion of [ENTER_MOTION, EXIT_MOTION]) {
      expect(motion.duration).toMatch(/^var\(--dur-\d, /u);
      expect(motion.easing).toMatch(/^var\(--ease-[a-z]+, /u);
    }
  });
});

describe("the reveal scale", () => {
  it("is 1.06, the number §5 names", () => {
    expect(REVEAL_SCALE).toBe(1.06);
    expect(previewScale(after(enter, dwell))).toBe(1.06);
  });

  it("is hard-capped at 1.12 so adjacent tiles cannot collide", () => {
    expect(REVEAL_SCALE_CAP).toBe(1.12);
    expect(revealScale(1.12)).toBe(1.12);
    expect(revealScale(1.2)).toBe(1.12);
  });

  it("never shrinks a card below its own size", () => {
    expect(revealScale(1)).toBe(1);
    expect(revealScale(0.5)).toBe(1);
    expect(revealScale(0)).toBe(1);
  });
});

describe("the lift that puts the revealed tile over its neighbours", () => {
  it("is taken by a revealed tile, and kept by one still shrinking back", () => {
    // The defect this answers, measured in the gallery: every tile's `z-index` was `auto`, so a tile
    // with a later sibling had half its ramp painted *under* that sibling's poster. The exit keeps
    // the lift because the tile is still enlarged while it shrinks — releasing it at `pointerLeave`
    // would cut the ramp in two for the length of the exit.
    expect(previewLift(IDLE_PREVIEW)).toBeNull();
    expect(previewLift(after(enter))).toBeNull();
    expect(previewLift(after(enter, dwell))).toBe(REVEAL_LIFT_Z);

    const leaving = after(enter, dwell, { type: "pointerLeave" });
    expect(leaving.animating).toBe(true);
    expect(previewLift(leaving)).toBe(REVEAL_LIFT_Z);
    expect(previewLift(after(enter, dwell, { type: "pointerLeave" }, { type: "settled" }))).toBeNull();
  });

  it("is positive, so it paints after every tile that keeps document order", () => {
    // A zero or negative index would leave the revealed tile in the same painting step as its
    // neighbours, which is the order the lift exists to escape.
    expect(REVEAL_LIFT_Z).toBeGreaterThan(0);
  });
});

describe("the room a rail has to leave", () => {
  it("covers the scale's own growth, the focus ring, and the ramp on every side", () => {
    // A static render cannot measure a clip box, so the arithmetic is pinned here and the rail's own
    // test holds its scroller to what this answers. `PG-E7-05`'s defect was the scroller clipping the
    // enlarged tile, the ring, and the ramp that is the lift.
    const width = 216;
    const height = 324;
    const room = revealRoom(width, height);

    expect(room.sides).toBeGreaterThanOrEqual(
      (width * (REVEAL_SCALE - 1)) / 2 + FOCUS_RING_OUTSET_PX,
    );
    expect(room.top).toBeGreaterThanOrEqual(
      (height * (REVEAL_SCALE - 1)) / 2 + FOCUS_RING_OUTSET_PX,
    );
    expect(room.bottom).toBeGreaterThanOrEqual(
      (height * (REVEAL_SCALE - 1)) / 2 + RAMP_OFFSET_Y_PX + RAMP_BLUR_PX,
    );
  });

  it("reaches further below than above, because the ramp falls", () => {
    const room = revealRoom(144, 216);

    expect(room.bottom).toBeGreaterThan(room.top);
    expect(room.top).toBeGreaterThan(0);
    expect(room.sides).toBeGreaterThan(0);
  });

  it("grows with the tile, and with a scale past the one §5 draws today", () => {
    // The rail sizes the room from the largest tile it can be asked to draw, and a variant that
    // revealed at the cap needs more room than this one — which is why the scale is a parameter
    // rather than a constant read inside.
    expect(revealRoom(216, 324).sides).toBeGreaterThan(revealRoom(112, 168).sides);
    expect(revealRoom(144, 216, REVEAL_SCALE_CAP).sides).toBeGreaterThan(
      revealRoom(144, 216).sides,
    );
  });

  it("is whole pixels on every side, because a fraction of a pixel is not room", () => {
    for (const width of [88, 112, 144, 176, 216]) {
      const room = revealRoom(width, (width * 3) / 2);
      for (const side of [room.sides, room.top, room.bottom]) {
        expect(Number.isInteger(side), String(width)).toBe(true);
      }
    }
  });
});
