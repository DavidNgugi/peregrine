/**
 * The hover-preview ladder (DESIGN_SYSTEM §5), as a pure state machine for the web client.
 *
 * This is a copy of `packages/ui/src/hover-preview.ts` rather than an import: that package is
 * Tailwind-bound and the browser client shares none of its build. Only the rules the browser's
 * poster tiles need are carried over, so the two cannot drift on the numbers or on what cancels.
 *
 * - A dwell, not a hover: enter arms {@link DWELL_MS} and the reveal happens only when it elapses.
 * - Leave, scroll, a hidden document, and falling below {@link MIN_VISIBLE_RATIO} visibility all
 *   cancel, because the tile is no longer what the pointer is over in each case.
 * - The reveal is capped by {@link REVEAL_SCALE_CAP}; a tile that grew further would obscure too
 *   much of its neighbours.
 * - Focus is keyboard parity, not a preview: it shows the panel and never scales the tile.
 */

/** How long the pointer must rest on a tile before the preview starts. */
export const DWELL_MS = 450;

/** How much the tile grows once the dwell has elapsed. */
export const REVEAL_SCALE = 1.3;

/** The hard ceiling on the reveal, which bounds the overlap with adjacent tiles. */
export const REVEAL_SCALE_CAP = 1.3;

/** Below this share of the tile on screen, the pointer is no longer reliably over it. */
export const MIN_VISIBLE_RATIO = 0.6;

/**
 * Where the ladder is.
 *
 * `dwelling` is the 450 ms in which a pointer crossing the tile still produces nothing;
 * `revealed` is the enlarged tile with its metadata panel.
 */
export type PreviewPhase = "idle" | "dwelling" | "revealed";

/**
 * The phase, and whether a transform transition is in flight.
 *
 * `animating` is what the card uses for `will-change` and for the stacking lift during the exit:
 * a dwelling card has no transition running, while a card returning from a reveal is still enlarged
 * until `transitionend` settles it.
 */
export interface PreviewState {
  readonly phase: PreviewPhase;
  readonly animating: boolean;
}

/** Nothing has happened: no dwell, no reveal, nothing to promote. */
export const IDLE_PREVIEW: PreviewState = { animating: false, phase: "idle" };

/**
 * Everything that can move the ladder.
 *
 * Each cancelling source is its own event rather than one `cancel`, so a test can hold every rule
 * separately — a single event would let three of them be unwired without a test noticing.
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
      // Re-entering a tile that is already dwelling or revealed must not restart the preview.
      return state.phase === "idle" ? { animating: true, phase: "dwelling" } : state;
    case "dwellElapsed":
      // The timer is the only way in, and a second timer for a reveal already up changes nothing.
      return state.phase === "dwelling" ? { animating: true, phase: "revealed" } : state;
    case "settled":
      // `transitionend`: whatever was animating has stopped, so the promotion comes off.
      return state.animating ? { animating: false, phase: state.phase } : state;
    case "pointerLeave":
    case "scroll":
      return cancelled(state);
    case "visibilityChange":
      return event.hidden ? cancelled(state) : state;
    case "intersection":
      // §5 says "dropping below 60%", so exactly 60% is still visible enough.
      return event.ratio < MIN_VISIBLE_RATIO ? cancelled(state) : state;
  }
}

/**
 * The cancel shared by a leave, a scroll, a hidden document, and a tile below the floor.
 *
 * A cancelled dwell is not an animation, so it must not leave `will-change` on. A cancelled reveal
 * is, because the exit transition is exactly what the cancel starts, so the promotion stays until
 * `settled`.
 */
function cancelled(state: PreviewState): PreviewState {
  return { animating: state.phase === "revealed", phase: "idle" };
}

/**
 * How much the tile is scaled right now.
 *
 * `lift` is whether the container reserved room for the enlargement. The grids do now — their
 * tracks overflow visibly and leave padding for the growth — so the default is true; a caller that
 * could not reserve room passes false and gets the panel without the scale.
 */
export function previewScale(state: PreviewState, lift = true): number {
  if (!lift || state.phase !== "revealed") {
    return 1;
  }
  return revealScale(REVEAL_SCALE);
}

/** The cap, applied; exported so the test fails if the ceiling moves. */
export function revealScale(requested: number): number {
  return Math.min(Math.max(requested, 1), REVEAL_SCALE_CAP);
}

/**
 * Whether the metadata panel is drawn.
 *
 * Two ways in, and they are different interactions: the dwell reveals it inside a scaled tile, and
 * focus reveals it inside a tile that stays where it is.
 */
export function panelVisible(state: PreviewState, focused: boolean): boolean {
  return state.phase === "revealed" || focused;
}
