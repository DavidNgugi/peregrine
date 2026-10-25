/**
 * The post-play coordinator: the one state machine that decides what happens when an episode or a
 * film ends.
 *
 * It owns the countdown, the cancel that must never start a follow-up the viewer backed out of,
 * the watch-credits and replay choices, and the prefetch outcome the caller uses to decide
 * between a ready next episode and a normal load. It owns no timer and performs no I/O: the shell
 * feeds {@link PostPlayEvent}s and executes the returned command.
 */

import type { NextUp } from "./player.js";

/** How long the next-up card waits before the follow-up starts. */
export const POST_PLAY_COUNTDOWN_MS = 10_000;

/** Whether the panel is absent, counting down, or waiting on a choice. */
export type PostPlayPhase = "hidden" | "countdown" | "ended";

/** How the next episode's prefetch is progressing. */
export type PostPlayPrefetch = "idle" | "loading" | "ready" | "failed";

/** What the post-play surface draws. */
export interface PostPlayState {
  readonly phase: PostPlayPhase;
  readonly remainingMs: number;
  readonly nextUp: NextUp | null;
  readonly prefetch: PostPlayPrefetch;
}

/** Everything the coordinator reacts to. */
export type PostPlayEvent =
  /** The caller has a next-up answer and wants the surface opened now. */
  | { readonly type: "open"; readonly nextUp: NextUp | null; readonly countdownMs: number }
  | { readonly type: "prefetch"; readonly outcome: PostPlayPrefetch }
  | { readonly type: "tick"; readonly elapsedMs: number }
  | { readonly type: "cancel" }
  | { readonly type: "start-next" }
  | { readonly type: "replay" }
  | { readonly type: "credits" }
  | { readonly type: "ended" };

/** What the shell must do after a transition; `null` means the state changed nothing. */
export type PostPlayCommand = "start-next" | "replay" | "credits" | "close";

/** The next state and the one action the shell should take for it. */
export interface PostPlayOutcome {
  readonly state: PostPlayState;
  readonly command: PostPlayCommand | null;
}

/** A session that has not reached the end of anything yet. */
export function initialPostPlayState(): PostPlayState {
  return { phase: "hidden", remainingMs: 0, nextUp: null, prefetch: "idle" };
}

function hidden(): PostPlayState {
  return { ...initialPostPlayState() };
}

function outcome(state: PostPlayState, command: PostPlayCommand | null): PostPlayOutcome {
  return { state, command };
}

function unchanged(state: PostPlayState): PostPlayOutcome {
  return outcome(state, null);
}

/**
 * A countdown the shell can actually wait for.
 *
 * A negative or non-finite duration is a caller bug, and the constant is the only answer that
 * still gives the viewer the cancellation window they were promised.
 */
function countdownMs(value: number): number {
  return Number.isFinite(value) && value >= 0 ? value : POST_PLAY_COUNTDOWN_MS;
}

/**
 * The next post-play state and the command it implies.
 *
 * **Exactly one start.** The zero tick moves to `hidden` and returns `start-next`; every later
 * tick sees `hidden` and does nothing. That is stronger than a separate "already started" flag:
 * there is no state in which a second start can be produced.
 *
 * **Cancel is final for this session.** It clears `nextUp` as well as hiding the panel, so even a
 * late credits or end event cannot resurrect a follow-up the viewer declined. A cancelled session
 * can still draw the ended panel, but it can never start anything.
 *
 * **A film stays on the panel.** With no `nextUp`, both an end and a credits marker land on the
 * ended panel; there is no black screen between the last frame and the choice.
 *
 * **Prefetch failure is not a dead end.** The outcome is recorded and the next episode is kept, so
 * zero still emits `start-next`; the caller falls back to a normal load when `prefetch` is not
 * `ready`.
 */
export function advance(state: PostPlayState, event: PostPlayEvent): PostPlayOutcome {
  switch (event.type) {
    case "open": {
      const duration = countdownMs(event.countdownMs);
      if (event.nextUp === null) {
        return unchanged({
          phase: "ended",
          remainingMs: 0,
          nextUp: null,
          prefetch: "idle",
        });
      }
      // A hidden state may already carry the prefetch of the episode about to be offered; only a
      // visible panel reopening for a new countdown discards it.
      const prefetch = state.phase === "hidden" ? state.prefetch : "idle";
      return unchanged({
        phase: "countdown",
        remainingMs: duration,
        nextUp: event.nextUp,
        prefetch,
      });
    }
    case "prefetch":
      return unchanged({ ...state, prefetch: event.outcome });
    case "tick": {
      if (state.phase !== "countdown") {
        return unchanged(state);
      }
      if (!Number.isFinite(event.elapsedMs) || event.elapsedMs < 0) {
        return unchanged(state);
      }
      const remainingMs = state.remainingMs - event.elapsedMs;
      if (remainingMs > 0) {
        return unchanged({ ...state, remainingMs });
      }
      return state.nextUp === null
        ? unchanged({ phase: "ended", remainingMs: 0, nextUp: null, prefetch: "idle" })
        : outcome(hidden(), "start-next");
    }
    case "cancel":
      return state.phase === "hidden" ? unchanged(state) : unchanged(hidden());
    case "start-next":
      if (state.nextUp === null) {
        return unchanged(state);
      }
      if (state.phase !== "countdown" && state.phase !== "ended") {
        return unchanged(state);
      }
      return outcome(hidden(), "start-next");
    case "replay":
      return state.phase === "hidden" ? unchanged(state) : outcome(hidden(), "replay");
    case "credits":
      // The phase disambiguates the two meanings of the same gesture: while the panel is hidden
      // a credits marker has been reached, and while it is visible the viewer chose Watch
      // credits. The shell can therefore report the marker and the button through one event.
      return state.phase === "hidden" ? openFromMarker(state) : outcome(hidden(), "credits");
    case "ended":
      return openFromMarker(state);
  }
}

/**
 * Opens the countdown from a marker or the real end, reusing the next-up the state already holds.
 *
 * Unlike `open`, this path has no fresh metadata: the caller is reporting that the media reached
 * a boundary, so a second report while the panel is up must not restart the countdown.
 */
function openFromMarker(state: PostPlayState): PostPlayOutcome {
  if (state.phase !== "hidden") {
    return unchanged(state);
  }
  if (state.nextUp === null) {
    return unchanged({ phase: "ended", remainingMs: 0, nextUp: null, prefetch: "idle" });
  }
  return unchanged({
    phase: "countdown",
    remainingMs: POST_PLAY_COUNTDOWN_MS,
    nextUp: state.nextUp,
    prefetch: state.prefetch,
  });
}
