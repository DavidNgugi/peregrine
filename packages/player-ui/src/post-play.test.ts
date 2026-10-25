import { describe, expect, it } from "vitest";
import { advance, initialPostPlayState, POST_PLAY_COUNTDOWN_MS } from "./post-play.js";
import type { PostPlayPrefetch, PostPlayState } from "./post-play.js";
import type { NextUp } from "./player.js";

const NEXT_UP: NextUp = {
  titleId: 7,
  seasonNumber: 2,
  episodeNumber: 3,
  episodeId: 42,
  name: "The Long Way Down",
};

function hiddenState(
  nextUp: NextUp | null = null,
  prefetch: PostPlayPrefetch = "idle",
): PostPlayState {
  return { phase: "hidden", remainingMs: 0, nextUp, prefetch };
}

function countdownState(
  remainingMs: number = POST_PLAY_COUNTDOWN_MS,
  prefetch: PostPlayPrefetch = "idle",
): PostPlayState {
  return { phase: "countdown", remainingMs, nextUp: NEXT_UP, prefetch };
}

function endedState(
  nextUp: NextUp | null = NEXT_UP,
  prefetch: PostPlayPrefetch = "idle",
): PostPlayState {
  return { phase: "ended", remainingMs: 0, nextUp, prefetch };
}

describe("initialPostPlayState", () => {
  it("starts hidden with nothing to offer", () => {
    expect(initialPostPlayState()).toEqual({
      phase: "hidden",
      remainingMs: 0,
      nextUp: null,
      prefetch: "idle",
    });
  });

  it("returns a fresh value so one session cannot write another's state", () => {
    expect(initialPostPlayState()).not.toBe(initialPostPlayState());
  });
});

describe("open", () => {
  it("counts down when a next episode exists", () => {
    const result = advance(initialPostPlayState(), {
      type: "open",
      nextUp: NEXT_UP,
      countdownMs: 5_000,
    });

    expect(result.state).toEqual({
      phase: "countdown",
      remainingMs: 5_000,
      nextUp: NEXT_UP,
      prefetch: "idle",
    });
    expect(result.command).toBeNull();
  });

  it("shows the ended panel for a film instead of a black screen", () => {
    const result = advance(initialPostPlayState(), {
      type: "open",
      nextUp: null,
      countdownMs: 5_000,
    });

    expect(result.state).toEqual({
      phase: "ended",
      remainingMs: 0,
      nextUp: null,
      prefetch: "idle",
    });
    expect(result.command).toBeNull();
  });

  it("keeps the whole countdown when the duration is unusable", () => {
    for (const countdownMs of [-1, Number.NaN, Number.POSITIVE_INFINITY]) {
      const result = advance(initialPostPlayState(), {
        type: "open",
        nextUp: NEXT_UP,
        countdownMs,
      });
      expect(result.state.remainingMs).toBe(POST_PLAY_COUNTDOWN_MS);
    }
  });

  it("carries a prefetch that finished while the panel was hidden", () => {
    const result = advance(hiddenState(null, "ready"), {
      type: "open",
      nextUp: NEXT_UP,
      countdownMs: 2_000,
    });
    expect(result.state.prefetch).toBe("ready");
  });

  it("starts a new prefetch when a visible panel reopens", () => {
    const result = advance(countdownState(POST_PLAY_COUNTDOWN_MS, "ready"), {
      type: "open",
      nextUp: NEXT_UP,
      countdownMs: 2_000,
    });
    expect(result.state.prefetch).toBe("idle");
  });

  it("starts a zero countdown without firing before the first tick", () => {
    const result = advance(initialPostPlayState(), {
      type: "open",
      nextUp: NEXT_UP,
      countdownMs: 0,
    });
    expect(result.state.phase).toBe("countdown");
    expect(result.state.remainingMs).toBe(0);
    expect(result.command).toBeNull();
  });
});

describe("credits and end of media", () => {
  it("opens the countdown from a credits marker when a next episode exists", () => {
    const result = advance(hiddenState(NEXT_UP, "ready"), { type: "credits" });
    expect(result.state).toEqual({
      phase: "countdown",
      remainingMs: POST_PLAY_COUNTDOWN_MS,
      nextUp: NEXT_UP,
      prefetch: "ready",
    });
    expect(result.command).toBeNull();
  });

  it("opens the ended panel from a credits marker for a film", () => {
    const result = advance(initialPostPlayState(), { type: "credits" });
    expect(result.state).toEqual({
      phase: "ended",
      remainingMs: 0,
      nextUp: null,
      prefetch: "idle",
    });
    expect(result.command).toBeNull();
  });

  it("opens the countdown from the end of media when a next episode exists", () => {
    const result = advance(hiddenState(NEXT_UP), { type: "ended" });
    expect(result.state.phase).toBe("countdown");
    expect(result.state.remainingMs).toBe(POST_PLAY_COUNTDOWN_MS);
    expect(result.state.nextUp).toEqual(NEXT_UP);
    expect(result.command).toBeNull();
  });

  it("opens the ended panel from the end of media for a film", () => {
    const result = advance(initialPostPlayState(), { type: "ended" });
    expect(result.state.phase).toBe("ended");
    expect(result.state.nextUp).toBeNull();
    expect(result.command).toBeNull();
  });

  it("does not restart a countdown that is already running", () => {
    const state = countdownState(4_000);
    expect(advance(state, { type: "ended" })).toEqual({ state, command: null });
  });

  it("does not reopen an ended panel", () => {
    const state = endedState();
    expect(advance(state, { type: "ended" })).toEqual({ state, command: null });
    expect(advance(state, { type: "credits" })).toEqual({
      state: hiddenState(),
      command: "credits",
    });
  });
});

describe("countdown", () => {
  it("counts down without starting anything early", () => {
    const result = advance(countdownState(), { type: "tick", elapsedMs: 4_000 });
    expect(result.state.remainingMs).toBe(6_000);
    expect(result.state.phase).toBe("countdown");
    expect(result.command).toBeNull();
  });

  it("fires exactly one start-next at zero and never again", () => {
    let state = countdownState();
    const commands: (string | null)[] = [];

    for (let tick = 0; tick < 30; tick += 1) {
      const step = advance(state, { type: "tick", elapsedMs: 1_000 });
      state = step.state;
      commands.push(step.command);
    }

    expect(commands.filter((command) => command === "start-next")).toHaveLength(1);
    expect(state).toEqual(hiddenState());
  });

  it("fires at exactly zero", () => {
    const result = advance(countdownState(0), { type: "tick", elapsedMs: 0 });
    expect(result.command).toBe("start-next");
    expect(result.state.phase).toBe("hidden");
  });

  it("fires when a tick overshoots the remaining time", () => {
    const result = advance(countdownState(1_000), { type: "tick", elapsedMs: 99_999 });
    expect(result.command).toBe("start-next");
    expect(result.state).toEqual(hiddenState());
  });

  it("ignores a negative or non-finite elapsed time", () => {
    for (const elapsedMs of [-1, Number.NaN, Number.POSITIVE_INFINITY]) {
      const state = countdownState(0);
      expect(advance(state, { type: "tick", elapsedMs })).toEqual({ state, command: null });
    }
  });

  it("ignores a tick while hidden or ended", () => {
    for (const state of [hiddenState(), endedState()]) {
      expect(advance(state, { type: "tick", elapsedMs: 99_999 })).toEqual({
        state,
        command: null,
      });
    }
  });

  it("still starts when the prefetch failed, leaving the caller to load normally", () => {
    let state = countdownState(1_000);
    state = advance(state, { type: "prefetch", outcome: "failed" }).state;
    expect(state.prefetch).toBe("failed");
    expect(state.nextUp).toEqual(NEXT_UP);

    const result = advance(state, { type: "tick", elapsedMs: 1_000 });
    expect(result.command).toBe("start-next");
  });
});

describe("cancel", () => {
  it("hides a countdown without a command and clears the offer", () => {
    const result = advance(countdownState(), { type: "cancel" });
    expect(result.state).toEqual(hiddenState());
    expect(result.command).toBeNull();
  });

  it("hides an ended panel without a command", () => {
    const result = advance(endedState(), { type: "cancel" });
    expect(result.state).toEqual(hiddenState());
    expect(result.command).toBeNull();
  });

  it("is a no-op once already hidden", () => {
    const state = hiddenState();
    expect(advance(state, { type: "cancel" })).toEqual({ state, command: null });
  });

  it("never starts after a cancellation, whatever arrives next", () => {
    let state = advance(countdownState(), { type: "cancel" }).state;

    const sequence = [
      { type: "tick" as const, elapsedMs: 99_999 },
      { type: "start-next" as const },
      { type: "ended" as const },
      { type: "tick" as const, elapsedMs: 99_999 },
      { type: "start-next" as const },
      { type: "credits" as const },
      { type: "tick" as const, elapsedMs: 99_999 },
      { type: "start-next" as const },
    ];

    for (const event of sequence) {
      const step = advance(state, event);
      state = step.state;
      expect(step.command).not.toBe("start-next");
    }
  });
});

describe("start-next", () => {
  it("starts from the countdown and can only be asked once", () => {
    const first = advance(countdownState(), { type: "start-next" });
    expect(first.state).toEqual(hiddenState());
    expect(first.command).toBe("start-next");

    const second = advance(first.state, { type: "start-next" });
    expect(second.command).toBeNull();
    expect(second.state).toEqual(hiddenState());
  });

  it("starts from the ended panel when a next episode exists", () => {
    const result = advance(endedState(), { type: "start-next" });
    expect(result.state).toEqual(hiddenState());
    expect(result.command).toBe("start-next");
  });

  it("does nothing without a next episode", () => {
    const state = endedState(null);
    expect(advance(state, { type: "start-next" })).toEqual({ state, command: null });
  });

  it("does nothing while hidden, even if a next-up is still in the state", () => {
    const state = hiddenState(NEXT_UP);
    expect(advance(state, { type: "start-next" })).toEqual({ state, command: null });
  });
});

describe("prefetch", () => {
  it("records every outcome without moving the phase or issuing a command", () => {
    for (const outcome of ["loading", "ready", "failed"] as const) {
      const state = countdownState();
      const result = advance(state, { type: "prefetch", outcome });
      expect(result.state.prefetch).toBe(outcome);
      expect(result.state.phase).toBe("countdown");
      expect(result.state.remainingMs).toBe(state.remainingMs);
      expect(result.state.nextUp).toEqual(NEXT_UP);
      expect(result.command).toBeNull();
    }
  });

  it("does not drop next-up when the prefetch fails", () => {
    const result = advance(countdownState(), { type: "prefetch", outcome: "failed" });
    expect(result.state.nextUp).toEqual(NEXT_UP);
  });

  it("is recorded while the panel is hidden too", () => {
    const result = advance(hiddenState(), { type: "prefetch", outcome: "ready" });
    expect(result.state.prefetch).toBe("ready");
    expect(result.state.phase).toBe("hidden");
  });
});

describe("replay and watch credits", () => {
  it("returns hidden with replay from the countdown", () => {
    const result = advance(countdownState(), { type: "replay" });
    expect(result.state).toEqual(hiddenState());
    expect(result.command).toBe("replay");
  });

  it("returns hidden with replay from the ended panel", () => {
    const result = advance(endedState(), { type: "replay" });
    expect(result.state).toEqual(hiddenState());
    expect(result.command).toBe("replay");
  });

  it("returns hidden with credits from the countdown and the ended panel", () => {
    for (const state of [countdownState(), endedState()]) {
      const result = advance(state, { type: "credits" });
      expect(result.state).toEqual(hiddenState());
      expect(result.command).toBe("credits");
    }
  });

  it("issues each choice once", () => {
    for (const event of [{ type: "replay" as const }, { type: "credits" as const }]) {
      const first = advance(endedState(), event);
      expect(first.command).not.toBeNull();
      expect(advance(first.state, event).command).toBeNull();
    }
  });

  it("does nothing while hidden", () => {
    const state = hiddenState();
    expect(advance(state, { type: "replay" })).toEqual({ state, command: null });
  });
});

describe("a film", () => {
  it("ends on the panel and never on a black screen", () => {
    const result = advance(initialPostPlayState(), { type: "ended" });
    expect(result.state.phase).toBe("ended");
    expect(result.state.nextUp).toBeNull();
  });

  it("cannot start a follow-up from the panel", () => {
    const state = endedState(null);
    expect(advance(state, { type: "start-next" }).command).toBeNull();
    expect(advance(state, { type: "tick", elapsedMs: 99_999 }).command).toBeNull();
  });

  it("can still replay or watch credits", () => {
    expect(advance(endedState(null), { type: "replay" }).command).toBe("replay");
    expect(advance(endedState(null), { type: "credits" }).command).toBe("credits");
  });
});

describe("purity", () => {
  it("does not mutate the state it was given", () => {
    const state = countdownState(4_000, "loading");
    const before = structuredClone(state);
    advance(state, { type: "tick", elapsedMs: 1_000 });
    advance(state, { type: "prefetch", outcome: "ready" });
    advance(state, { type: "cancel" });
    expect(state).toEqual(before);
  });
});
