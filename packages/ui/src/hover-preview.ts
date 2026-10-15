/**
 * The hover-preview ladder (`DESIGN_SYSTEM §5`), as a state machine rather than as event handlers.
 *
 * The ladder is the most scrutinized interaction in the app and the one a test environment cannot
 * exercise: the test runner here is Node with `react-dom/server`, so there is no pointer, no timer,
 * and no observer callback to deliver (`PG-E21-03`'s named gap). What a component test *can* do is
 * render the markup a state produces — which means the state has to be reachable without the DOM.
 * That is why the whole ladder lives in this file as a pure function: `nextPreview` is the
 * transition, `previewScale` is the transform, and `panelVisible` is the metadata layer, and the
 * card's only job is to hand `nextPreview` the events its listeners received.
 *
 * The rules §5 states, and where each one is decided:
 *
 * - **A dwell, not a hover.** Enter arms a {@link DWELL_MS} delay and nothing else; the reveal
 *   happens when that delay elapses, and it never happens without it or twice for one enter.
 * - **Anything that moves the card out from under the pointer cancels.** Leave, scroll, the document
 *   going hidden, and dropping below {@link MIN_VISIBLE_RATIO} of visibility all cancel — the last
 *   two because the card is no longer what the pointer is over even though the pointer never moved.
 * - **The scale is capped.** {@link REVEAL_SCALE_CAP} is the hard ceiling, because a tile that grows
 *   past it obscures too much of its neighbours.
 * - **Focus is not a preview.** A focused card shows the metadata panel and nothing else, which is
 *   why {@link panelVisible} takes the focus separately and {@link previewScale} does not.
 */

/** §5's dwell: how long the pointer must rest on a tile before the preview is worth starting. */
export const DWELL_MS = 450;

/** §5's reveal: how much the tile grows once the dwell has elapsed. */
export const REVEAL_SCALE = 1.3;

/** §5's hard cap, which limits the overlap with adjacent tiles. */
export const REVEAL_SCALE_CAP = 1.3;

/** §5's visibility floor: below 60% of the tile on screen, the preview is cancelled. */
export const MIN_VISIBLE_RATIO = 0.6;

/**
 * The stacking order a revealed tile takes over the tiles it overlaps.
 *
 * A tile is `position: relative` and every tile's `z-index` is otherwise `auto`, so the two
 * neighbours and the revealed tile paint in document order — which means the ramp of a tile with a
 * later sibling is painted *under* that sibling, and half the lift disappears. A positive index puts
 * the revealed tile in the step after every `auto` one, so its ramp falls across both neighbours.
 */
export const REVEAL_LIFT_Z = 10;

/**
 * §5's shadow ramp, as the two numbers a rail needs in order to leave it room.
 *
 * The ramp is a `box-shadow` of `0 12px 32px` on a pseudo-element that is `inset-0` of the tile, so
 * it reaches 32 − 12 = 20 px above the tile and 12 + 32 = 44 px below it. These live here rather
 * than only in the card's class because {@link revealRoom} has to answer with a number, and a number
 * copied out of a class name is a number nobody updates.
 */
export const RAMP_BLUR_PX = 32;

/** How far the ramp is pushed down, so more of it falls below the tile than above. */
export const RAMP_OFFSET_Y_PX = 12;

/** §4's focus ring: a 2 px outline at a 2 px offset, so the ring reaches 4 px past the tile. */
export const FOCUS_RING_OUTSET_PX = 4;

/**
 * How far an enlarged tile reaches past its own box, and so how much room a rail has to leave it.
 *
 * **This is the rail's problem and not the tile's**, which is why it is arithmetic in this module
 * rather than a constant in `Rail`: the tile's transform does not move anything, so a scroll
 * container that clips — and `overflow-x: auto` computes `overflow-y` to `auto`, so every rail's
 * scroller clips on both axes — cuts the enlarged tile, its focus ring, and the ramp that is the
 * whole of the lift. A rail that left no room was a card the same size as its neighbours with the
 * metadata panel drawn inside it, reported from a screenshot.
 */
export interface RevealRoom {
  /** How far past the tile the room reaches on the left and the right. */
  readonly sides: number;
  readonly top: number;
  readonly bottom: number;
}

/**
 * The room a tile of `width` × `height` needs around it, at {@link REVEAL_SCALE}.
 *
 * Rounded up, because a fractional pixel of room is not a pixel of room. The default scale is the one
 * §5 draws; a variant that revealed at {@link REVEAL_SCALE_CAP} would need more, and the `scale`
 * parameter exists so that caller can ask for it rather than discovering it in a screenshot.
 */
export function revealRoom(
  width: number,
  height: number,
  scale: number = REVEAL_SCALE,
): RevealRoom {
  const grown = revealScale(scale) - 1;
  const grownX = (width * grown) / 2;
  const grownY = (height * grown) / 2;
  return {
    sides: Math.ceil(grownX + RAMP_BLUR_PX + FOCUS_RING_OUTSET_PX),
    top: Math.ceil(grownY + RAMP_BLUR_PX - RAMP_OFFSET_Y_PX + FOCUS_RING_OUTSET_PX),
    bottom: Math.ceil(grownY + RAMP_OFFSET_Y_PX + RAMP_BLUR_PX + FOCUS_RING_OUTSET_PX),
  };
}

/**
 * Where the ladder is.
 *
 * `dwelling` is the 450 ms of nothing-that-shows that separates a pointer crossing a tile from a
 * pointer resting on it; `revealed` is the enlarged layer.
 */
export type PreviewPhase = "idle" | "dwelling" | "revealed";

/**
 * The phase, and whether a transform transition is in flight.
 *
 * `animating` is separate from `phase` because the two disagree in both directions and the
 * difference is what `will-change` follows: leaving a *dwelling* card animates nothing (no
 * transition was ever started), while leaving a *revealed* one runs the 120 ms exit — and §5 rule 3
 * requires `will-change` on the element that is animating and off every other tile in a grid.
 */
export interface PreviewState {
  readonly phase: PreviewPhase;
  /** Whether this card has a transform transition running, and so whether to promote it. */
  readonly animating: boolean;
}

/** Nothing has happened: no dwell, no reveal, nothing to promote. */
export const IDLE_PREVIEW: PreviewState = { animating: false, phase: "idle" };

/**
 * Everything that can move the ladder, named as the source names it.
 *
 * The four cancelling sources are events of their own rather than one `cancel`, so a test can hold
 * each of §5's listed triggers to the rule separately — a single event would let three of them be
 * unwired without a test noticing.
 */
export type PreviewEvent =
  | { readonly type: "pointerEnter" }
  | { readonly type: "pointerLeave" }
  | { readonly type: "dwellElapsed" }
  | { readonly type: "scroll" }
  | { readonly type: "visibilityChange"; readonly hidden: boolean }
  | { readonly type: "intersection"; readonly ratio: number }
  | { readonly type: "settled" };

/** The state an event leaves behind. */
export function nextPreview(state: PreviewState, event: PreviewEvent): PreviewState {
  switch (event.type) {
    case "pointerEnter":
      // A pointer that re-enters an already-revealed tile does not restart anything: the preview is
      // up, and re-arming a dwell would take it away and put it back.
      return state.phase === "idle" ? { animating: true, phase: "dwelling" } : state;
    case "dwellElapsed":
      // Only a dwell can fire the preview, and only once — a second timer arriving for a reveal
      // already up changes nothing.
      return state.phase === "dwelling" ? { animating: true, phase: "revealed" } : state;
    case "settled":
      // `transitionend`: whatever was animating has stopped, so the layer promotion comes off.
      return state.animating ? { animating: false, phase: state.phase } : state;
    case "pointerLeave":
    case "scroll":
      return cancelled(state);
    case "visibilityChange":
      return event.hidden ? cancelled(state) : state;
    case "intersection":
      // At exactly the floor the card is still visible enough (§5: "dropping below 60%"), so the
      // comparison is strict.
      return event.ratio < MIN_VISIBLE_RATIO ? cancelled(state) : state;
  }
}

/**
 * The cancel a leave, a scroll, a hidden document, and a tile falling below the floor all share.
 *
 * A dwell is not an animation: cancelling it must not leave `will-change` on for a transition that
 * never ran. A reveal *is* one — the exit transition is exactly what cancelling starts — so the
 * promotion stays until `settled`.
 */
function cancelled(state: PreviewState): PreviewState {
  return { animating: state.phase === "revealed", phase: "idle" };
}

/**
 * How much the card is scaled right now.
 *
 * §5's reveal scale, clamped so that a caller which asks for more than
 * {@link REVEAL_SCALE_CAP} — a future size variant, say — cannot put two tiles through each other,
 * and never scaled below 1: a card that shrank on hover would be a different interaction.
 *
 * **`lift` is whether the container reserved room for the enlargement, and `false` means it did not.**
 * A scroller clips on both axes — `overflow-y: auto` computes `overflow-x` to `auto` too — so an
 * enlarged tile is cut by whatever contains it unless that container left space for it. A rail does,
 * through {@link revealRoom}, and that is the whole reason the function exists. A **grid** cannot: its
 * column count is derived to fit the measured width, so room for a 30 % overhang on every tile is room
 * the grid would have to take out of its own columns. So a grid passes `false` and the panel reveals
 * *inside* the tile, which is what `truncate` on the panel's lines is for. Found from a screenshot: the
 * leftmost tile of a fresh library was drawn with its title and its action cut off at both sides.
 */
export function previewScale(state: PreviewState, lift = true): number {
  if (!lift || state.phase !== "revealed") {
    return 1;
  }
  return revealScale(REVEAL_SCALE);
}

/** The cap, applied. Exported for the test that would fail if the number moved. */
export function revealScale(requested: number): number {
  return Math.min(Math.max(requested, 1), REVEAL_SCALE_CAP);
}

/**
 * The stacking order the tile paints at right now, or `null` for document order.
 *
 * The lift follows the *size* rather than the phase: a revealed tile is enlarged, and a tile that is
 * animating back from a reveal is still enlarged while it shrinks — so it stays lifted until
 * `settled` says the transform stopped, or its ramp would be cut in half by its neighbour for the
 * length of the exit. A dwell is neither, so it lifts nothing.
 */
export function previewLift(state: PreviewState): number | null {
  return state.phase === "revealed" || (state.animating && state.phase === "idle")
    ? REVEAL_LIFT_Z
    : null;
}

/**
 * Whether the metadata layer is drawn.
 *
 * Two ways in and they are not the same interaction: the dwell reveals it inside a scaled card, and
 * focus reveals it inside a card that stays where it is (§5's keyboard-parity row — the preview is
 * pointer-only and must never trigger on focus).
 */
export function panelVisible(state: PreviewState, focused: boolean): boolean {
  return state.phase === "revealed" || focused;
}

/**
 * Whether the tile's own rating badge is drawn.
 *
 * The revealed layer repeats the score where the eye can read it, so the badge behind it is a
 * ghost of a second one and comes off while the panel is up — one score per tile, not two at
 * different contrasts. Found by looking at the gallery rather than by a test.
 *
 * The availability chip is deliberately not part of this: the panel does not repeat the state, and
 * the chip stays legible through the layer, which is exactly the moment a person wants to know what
 * is happening to the title.
 */
export function badgeVisible(state: PreviewState, focused: boolean): boolean {
  return !panelVisible(state, focused);
}

/** A transition's two CSS values, as the card writes them onto the element. */
export interface PreviewMotion {
  readonly duration: string;
  readonly easing: string;
}

/**
 * §5's enter: 180 ms to scale 1.30, decelerating into the stop.
 *
 * The values name the `--dur-*`/`--ease-*` tokens PG-E21-07 has not landed yet, with §5's own numbers
 * as the fallback — so the ladder is right today and picks the token up, reduced-motion collapse
 * included, the day it exists.
 */
export const ENTER_MOTION: PreviewMotion = {
  duration: "var(--dur-3, 180ms)",
  easing: "var(--ease-decelerate, cubic-bezier(0, 0, 0, 1))",
};

/** §5's exit: 120 ms back to scale 1, accelerating away. */
export const EXIT_MOTION: PreviewMotion = {
  duration: "var(--dur-2, 120ms)",
  easing: "var(--ease-accelerate, cubic-bezier(0.3, 0, 1, 1))",
};

/**
 * Which transition the card is on.
 *
 * Here rather than inline in the component for the reason the rest of the ladder is: the enter's
 * values only exist in the revealed state, which a `react-dom/server` test cannot reach, and a rule
 * about a duration that no test can see is a rule nobody is holding to §5.
 */
export function previewMotion(state: PreviewState): PreviewMotion {
  return state.phase === "revealed" ? ENTER_MOTION : EXIT_MOTION;
}
