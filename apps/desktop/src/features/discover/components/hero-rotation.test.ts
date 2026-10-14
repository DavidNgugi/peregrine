import { describe, expect, it } from "vitest";

import type { HomeRailDto, RailItemDto } from "@peregrine/ipc-client";
import {
  advance,
  currentSlide,
  FIRST_SLIDE,
  heroPlay,
  heroSlides,
  HERO_INTERVAL_MS,
  HERO_ROTATION_MAX,
  rotationRuns,
  selectSlide,
  type RotationState,
} from "./hero-rotation.js";

/**
 * The rotation's decisions, which is everything about it a Node test can hold.
 *
 * What this file *cannot* drive is the interval: `HERO_INTERVAL_MS` is a number here and a
 * `window.setInterval` in `DiscoverHero`, and `react-dom/server` has no timers at all. So the
 * schedule is asserted as a state machine — every move produces a new `epoch`, which is what the
 * component keys the timer on — and what the browser does with it is `DiscoverHero`'s, verified by
 * the pure decisions it hands over rather than by a test that waits.
 */
function item(titleId: number, overrides: Partial<RailItemDto> = {}): RailItemDto {
  return {
    titleId,
    kind: "movie",
    name: `Title ${String(titleId)}`,
    year: 1999,
    poster: null,
    backdrop: null,
    playable: false,
    rating: null,
    overview: null,
    votes: null,
    ...overrides,
  };
}

function rail(items: readonly RailItemDto[], name = "popular_movies"): HomeRailDto {
  return { rail: name, items: [...items], state: "ready" };
}

/** The rotation after a list of moves, from the lead slide. */
function after(count: number, ...moves: readonly ("advance" | number)[]): RotationState {
  return moves.reduce<RotationState>(
    (state, move) => (move === "advance" ? advance(state, count) : selectSlide(state, move, count)),
    FIRST_SLIDE,
  );
}

describe("which items rotate", () => {
  it("is the first rail that has any, capped, and nothing when none has", () => {
    // The same rule the core's `hero_backdrops` uses to decide whose wide images are worth
    // fetching, so the hero cannot ask for a backdrop nobody fetched.
    expect(heroSlides([])).toEqual([]);
    expect(heroSlides([rail([])])).toEqual([]);
    expect(heroSlides([rail([], "trending_week"), rail([item(4), item(5)])]).map((i) => i.titleId))
      .toEqual([4, 5]);
    expect(heroSlides([rail([item(1), item(2), item(3), item(4), item(5), item(6)])])).toHaveLength(
      HERO_ROTATION_MAX,
    );
    expect(HERO_ROTATION_MAX).toBe(5);
  });
});

describe("the rotation's arithmetic", () => {
  it("runs only when there is more than one slide to run to", () => {
    // One slide is not a rotation: the interval would re-arm to draw the same picture, and the strip
    // would be a control that does nothing (`PG-E1-07`).
    expect(rotationRuns(0)).toBe(false);
    expect(rotationRuns(1)).toBe(false);
    expect(rotationRuns(2)).toBe(true);
  });

  it("steps forward one slide", () => {
    expect(currentSlide(after(3, "advance"), 3)).toBe(1);
    expect(currentSlide(after(3, "advance", "advance"), 3)).toBe(2);
  });

  it("wraps at the end rather than stopping", () => {
    // A hero that stopped would be a still image with a strip beside it: the rotation is the point,
    // and the first slide is where a person who looked away picks the thread back up.
    expect(currentSlide(after(3, "advance", "advance", "advance"), 3)).toBe(0);
    expect(currentSlide(after(2, "advance", "advance"), 2)).toBe(0);
  });

  it("has nowhere to go with one slide or none, and says so by not moving", () => {
    // Returning the *same* state is what keeps a no-op from re-rendering and from implying a timer
    // the component must not arm.
    expect(advance(FIRST_SLIDE, 0)).toBe(FIRST_SLIDE);
    expect(advance(FIRST_SLIDE, 1)).toBe(FIRST_SLIDE);
    expect(selectSlide(FIRST_SLIDE, 3, 0)).toBe(FIRST_SLIDE);
  });

  it("brings a stored index back to a slide that exists", () => {
    // A refresh can replace the first rail with a shorter one under an index the state already
    // holds; a hero that indexed past its own slides would draw none of them.
    expect(currentSlide({ index: 9, epoch: 4 }, 3)).toBe(2);
    expect(currentSlide({ index: -2, epoch: 1 }, 3)).toBe(0);
    expect(currentSlide({ index: 9, epoch: 4 }, 0)).toBe(0);
  });
});

describe("what a click does to the schedule", () => {
  it("goes to the slide that was clicked, clamped to the strip that is drawn", () => {
    expect(currentSlide(after(5, 3), 5)).toBe(3);
    expect(currentSlide(after(5, 9), 5)).toBe(4);
    expect(currentSlide(after(5, -1), 5)).toBe(0);
  });

  it("re-arms the interval, so a click is answered rather than raced", () => {
    // The requirement stated as a move: a person clicking the fourth thumbnail with a tenth of a
    // second left in the interval must not watch it advance away from what they chose. The `epoch`
    // is what the component's timer depends on, so bumping it is exactly "a whole interval from the
    // click" — and it is bumped even when the slide clicked is already the one on screen.
    const before = after(5, "advance");
    const clicked = selectSlide(before, 0, 5);

    expect(clicked.epoch).toBe(before.epoch + 1);
    expect(selectSlide(before, before.index, 5).epoch).toBe(before.epoch + 1);
  });

  it("re-arms on the automatic move too, so a slide always gets its whole interval", () => {
    expect(advance(FIRST_SLIDE, 3).epoch).toBe(1);
    expect(after(3, "advance", "advance").epoch).toBe(2);
  });
});

describe("how long a slide stays up", () => {
  it("is eight seconds, which is between a skim and a careful read", () => {
    // The number is an argument rather than a round one (see the constant), and this is the one place
    // it is written twice: the component reads it, and a change to it has to be a change here.
    expect(HERO_INTERVAL_MS).toBe(8_000);
  });
});

describe("the hero's primary action", () => {
  it("plays only when there is a file and something that can play it", () => {
    expect(heroPlay(true, true)).toEqual({ kind: "play" });
  });

  it("names the title's own reason first, because that is the one a person can act on", () => {
    expect(heroPlay(false, true)).toEqual({ kind: "unavailable", reason: "noFile" });
    expect(heroPlay(false, false)).toEqual({ kind: "unavailable", reason: "noFile" });
  });

  it("says when the build has no player rather than drawing a control that fires nothing", () => {
    // This build's state: `PG-E15` owns playback and no surface for it exists, so the hero is handed
    // no handler and Play is disabled with a reason — never enabled and inert.
    expect(heroPlay(true, false)).toEqual({ kind: "unavailable", reason: "noPlayer" });
  });
});
