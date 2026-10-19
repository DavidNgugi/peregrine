import { describe, expect, it } from "vitest";

import {
  DWELL_MS,
  IDLE_PREVIEW,
  MIN_VISIBLE_RATIO,
  REVEAL_SCALE,
  REVEAL_SCALE_CAP,
  nextPreview,
  panelVisible,
  previewScale,
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
    expect(DWELL_MS).toBe(450);
  });

  it("starts on pointer enter without revealing anything yet", () => {
    const dwelling = after(enter);

    expect(dwelling.phase).toBe("dwelling");
    expect(previewScale(dwelling)).toBe(1);
    expect(panelVisible(dwelling, false)).toBe(false);
  });

  it("reveals only when the delay has elapsed", () => {
    expect(after(dwell).phase).toBe("idle");
    expect(after({ type: "settled" }).phase).toBe("idle");
    expect(after({ type: "scroll" }).phase).toBe("idle");
    expect(after(enter, dwell).phase).toBe("revealed");
  });

  it("fires once, however many times the timer arrives", () => {
    expect(after(enter, dwell, dwell)).toEqual(after(enter, dwell));
  });

  it("does not restart what is already revealed", () => {
    expect(after(enter, dwell, enter)).toEqual(after(enter, dwell));
  });
});

describe("what cancels the preview", () => {
  it("cancels a dwell on pointer leave", () => {
    expect(after(enter, { type: "pointerLeave" }).phase).toBe("idle");
  });

  it("cancels a dwell on scroll", () => {
    expect(after(enter, { type: "scroll" }).phase).toBe("idle");
  });

  it("cancels a dwell when the document goes hidden", () => {
    expect(after(enter, { type: "visibilityChange", hidden: true }).phase).toBe("idle");
  });

  it("keeps the dwell when the document becomes visible again", () => {
    expect(after(enter, { type: "visibilityChange", hidden: false }).phase).toBe("dwelling");
  });

  it("cancels below 60% visibility and keeps the dwell at exactly 60%", () => {
    expect(MIN_VISIBLE_RATIO).toBe(0.6);
    expect(after(enter, { type: "intersection", ratio: 0.59 }).phase).toBe("idle");
    expect(after(enter, { type: "intersection", ratio: 0.6 }).phase).toBe("dwelling");
    expect(after(enter, { type: "intersection", ratio: 1 }).phase).toBe("dwelling");
  });

  it("takes a revealed tile back down from any of the four", () => {
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

describe("the animation flag", () => {
  it("is on while a transition may run and off once it settles", () => {
    expect(after(enter).animating).toBe(true);
    expect(after(enter, dwell).animating).toBe(true);
    expect(after(enter, dwell, { type: "settled" })).toEqual({
      animating: false,
      phase: "revealed",
    });
  });

  it("stays on through the exit and comes off when the exit settles", () => {
    const leaving = after(enter, dwell, { type: "pointerLeave" });

    expect(leaving.phase).toBe("idle");
    expect(leaving.animating).toBe(true);
    expect(after(enter, dwell, { type: "pointerLeave" }, { type: "settled" })).toEqual(
      IDLE_PREVIEW,
    );
  });

  it("never turns on for a dwell cancelled before it fired", () => {
    expect(after(enter, { type: "scroll" }).animating).toBe(false);
    expect(after(enter, { type: "pointerLeave" }).animating).toBe(false);
  });
});

describe("the panel", () => {
  it("is drawn only once the dwell has elapsed", () => {
    expect(panelVisible(IDLE_PREVIEW, false)).toBe(false);
    expect(panelVisible(after(enter), false)).toBe(false);
    expect(panelVisible(after(enter, dwell), false)).toBe(true);
  });

  it("is drawn on focus, which never scales the tile", () => {
    expect(panelVisible(IDLE_PREVIEW, true)).toBe(true);
    expect(panelVisible(after(enter), true)).toBe(true);
    expect(previewScale(IDLE_PREVIEW)).toBe(1);
    expect(previewScale(after(enter))).toBe(1);
  });
});

describe("the reveal scale", () => {
  it("enlarges after the dwell and leaves every other phase alone", () => {
    expect(REVEAL_SCALE).toBe(1.3);
    expect(previewScale(after(enter, dwell))).toBe(1.3);
    expect(previewScale(IDLE_PREVIEW)).toBe(1);
    expect(previewScale(after(enter))).toBe(1);
  });

  it("stays at its own size when the container reserved no room", () => {
    const revealed = after(enter, dwell);

    expect(previewScale(revealed, false)).toBe(1);
    expect(previewScale(revealed, true)).toBe(1.3);
    expect(previewScale(IDLE_PREVIEW, false)).toBe(1);
  });

  it("caps the overlap with adjacent tiles at thirty percent", () => {
    expect(REVEAL_SCALE_CAP).toBe(1.3);
    expect(revealScale(1.3)).toBe(1.3);
    expect(revealScale(1.5)).toBe(1.3);
  });

  it("never shrinks a tile below its own size", () => {
    expect(revealScale(1)).toBe(1);
    expect(revealScale(0.5)).toBe(1);
    expect(revealScale(0)).toBe(1);
  });
});
