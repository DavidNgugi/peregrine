import type { HomeRailDto, RailItemDto } from "@peregrine/ipc-client";

/**
 * The hero's rotation, as a pure decision rather than as the component's effects (the
 * `hover-preview.ts` pattern, and for its reason: this runner is Node with `react-dom/server`, so
 * there is no timer to drive and a rule that lived inside `useEffect` could not be asserted at all).
 *
 * What is here is every question the rotation asks that has an answer: which items it cycles
 * (the first rail that has any, capped), which slide is next and what happens at the end of the
 * list, what a click on the strip does to the schedule, what a rail with one item or none does, and
 * whether the primary action can be honoured. What is *not* here is the interval itself — that is
 * `window.setInterval` in the component, keyed on {@link RotationState.epoch} — and the cross-fade,
 * which is CSS over §5's tokens.
 */

/**
 * How many of the first rail's items the hero rotates through.
 *
 * Five, and it is a bound rather than a guess: the core fetches a backdrop for the first eight items
 * of that rail (`HERO_BACKDROP_LIMIT`, which is this number with room for one that failed), and a
 * strip of thumbnails is a row a person reads at a glance — past about six it is a second rail
 * competing with the one below the hero for the same attention.
 */
export const HERO_ROTATION_MAX = 5;

/**
 * How long one slide stays up, in milliseconds.
 *
 * **Eight seconds, and the number is an argument rather than a round one.** The copy is a name, one
 * line of meta, a score with its count, and a synopsis clamped to three lines — about forty words,
 * which a careful reader takes around twelve seconds and a browser about four. Eight sits between
 * the two: long enough that a reader who is interested gets through most of it, short enough that a
 * browser does not wait through two slides they have already finished. It is also what keeps the
 * hero *stable* rather than animated — at two or three seconds it reads as motion and stops being
 * readable — and it is twenty-five times the whole-surface cross-fade it rides (§5's `--dur-5`, 320
 * ms), so the transition is a punctuation mark rather than a fifth of what is on screen.
 */
export const HERO_INTERVAL_MS = 8_000;

/**
 * Where the rotation is, and what the interval is keyed on.
 *
 * `epoch` is not a counter anyone draws: it is the value the timer's effect depends on, so *every*
 * move — the interval firing, or a click on the strip — tears the timer down and arms a fresh one.
 * That is what makes a click land as an answer rather than as a race: a person who clicks the fourth
 * thumbnail with a tenth of a second left in the interval would otherwise watch it advance away from
 * what they chose, which is a click that feels ignored.
 */
export interface RotationState {
  /** The slide's position in the rotation, before {@link currentSlide} clamps it. */
  readonly index: number;
  /** How many times the schedule has been re-armed. */
  readonly epoch: number;
}

/** Before anything has happened: the lead item, and a schedule nothing has reset. */
export const FIRST_SLIDE: RotationState = { index: 0, epoch: 0 };

/**
 * The items the hero rotates through: the first rail that has any, capped at
 * {@link HERO_ROTATION_MAX}.
 *
 * **The first rail with an item rather than the first rail**, which is the same rule the core's
 * `hero_backdrops` uses to decide whose backdrops are worth fetching: a provider answers what it
 * publishes, so the rail a home screen lists first is often empty, and a hero that waited for it
 * would show nothing while six rails of titles sat below it.
 */
export function heroSlides(rails: readonly HomeRailDto[]): readonly RailItemDto[] {
  for (const rail of rails) {
    if (rail.items.length > 0) {
      return rail.items.slice(0, HERO_ROTATION_MAX);
    }
  }
  return [];
}

/**
 * Whether there is anything to rotate.
 *
 * One item is not a rotation: the interval would re-arm for ever to show the same slide, and the
 * strip would be a row of one control that does nothing — absent rather than inert (`PG-E1-07`).
 */
export function rotationRuns(count: number): boolean {
  return count > 1;
}

/**
 * The slide on screen, clamped to the rotation that exists right now.
 *
 * A stored index and a shorter list is a real state — a refresh that replaced the first rail, an
 * empty answer — and a hero that indexed past its own slides would draw none of them.
 */
export function currentSlide(state: RotationState, count: number): number {
  if (count <= 0) {
    return 0;
  }
  return Math.min(Math.max(state.index, 0), count - 1);
}

/**
 * The next slide, wrapping at the end.
 *
 * **Wrapping rather than stopping**, because a hero that stopped at the last item would be a still
 * image with a strip beside it — the rotation is the point, and the first item is where a person who
 * looked away can pick the thread back up. A rotation of fewer than two items has nowhere to go and
 * is answered unchanged, so nothing re-renders and no timer is implied.
 */
export function advance(state: RotationState, count: number): RotationState {
  if (!rotationRuns(count)) {
    return state;
  }
  return { index: (currentSlide(state, count) + 1) % count, epoch: state.epoch + 1 };
}

/**
 * The slide a click on the strip asks for, and a schedule that starts again from the click.
 *
 * The index is clamped rather than trusted, because the strip and the state are two answers to the
 * same question and a list that shrank between them must not index past its end. Selecting the slide
 * that is *already up* still re-arms the interval: the click is a person saying "I am reading this
 * one", and the honest answer is a full {@link HERO_INTERVAL_MS} from the moment they said it.
 */
export function selectSlide(state: RotationState, index: number, count: number): RotationState {
  if (count <= 0) {
    return state;
  }
  return { index: Math.min(Math.max(index, 0), count - 1), epoch: state.epoch + 1 };
}

/** Why the hero's primary action cannot be taken. */
export type PlayReason = "noFile" | "noPlayer";

/**
 * What the hero's Play is: an action, or a control that is disabled for a stated reason.
 *
 * `noFile` is the core's fact about *this title* — the library holds no file for it — and it is
 * answered first, because it is the one a person can do something about. `noPlayer` is the build's
 * fact and is passed in rather than assumed: this build has no playback surface at all (`PG-E15`),
 * so no caller hands the hero a handler, and a control that fired nothing would be the inert
 * button `PG-E1-07` forbids.
 */
export type PlayAction =
  | { readonly kind: "play" }
  | { readonly kind: "unavailable"; readonly reason: PlayReason };

/** The action, from the two facts that decide it. */
export function heroPlay(playable: boolean, player: boolean): PlayAction {
  if (!playable) {
    return { kind: "unavailable", reason: "noFile" };
  }
  if (!player) {
    return { kind: "unavailable", reason: "noPlayer" };
  }
  return { kind: "play" };
}
