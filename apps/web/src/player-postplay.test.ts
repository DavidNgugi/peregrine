import { advance, POST_PLAY_COUNTDOWN_MS } from "@peregrine/player-ui";
import type { NextUp, PlayerMarker } from "@peregrine/player-ui";
import { describe, expect, it } from "vitest";

import type { Marker, NextEpisode } from "./api.js";
import {
  absolutePositionMs,
  hiddenPostPlay,
  nextUpLabel,
  skipTarget,
  toNextUp,
  toPlayerMarkers,
} from "./player-postplay.js";

const TICKET_MARKERS: readonly Marker[] = [
  { kind: "intro", startMs: 10_000, endMs: 70_000, source: "chapters", confidence: 100 },
  { kind: "recap", startMs: 70_000, endMs: 80_000, source: "introdb", confidence: 80 },
  {
    kind: "credits",
    startMs: 5_900_000,
    endMs: null,
    source: "introdb",
    confidence: 90,
  },
  {
    kind: "preview",
    startMs: 5_950_000,
    endMs: 5_990_000,
    source: "chapters",
    confidence: 100,
  },
];

const PLAYER_MARKERS: readonly PlayerMarker[] = [
  { id: 1, kind: "intro", startMs: 10_000, endMs: 70_000, source: "chapters", confidence: 100 },
  { id: 2, kind: "recap", startMs: 70_000, endMs: 80_000, source: "introdb", confidence: 80 },
  {
    id: 3,
    kind: "credits",
    startMs: 5_900_000,
    endMs: null,
    source: "introdb",
    confidence: 90,
  },
  {
    id: 4,
    kind: "preview",
    startMs: 5_950_000,
    endMs: 5_990_000,
    source: "chapters",
    confidence: 100,
  },
];

const NEXT_EPISODE: NextEpisode = {
  seasonNumber: 2,
  episodeNumber: 3,
  episodeId: 42,
  name: "The Long Way Down",
};

describe("toPlayerMarkers", () => {
  it("keeps the ticket's fields and gives each window a stable id", () => {
    const mapped = toPlayerMarkers(TICKET_MARKERS);
    expect(mapped).toEqual(PLAYER_MARKERS);
    expect(mapped.map((marker) => marker.id)).toEqual([1, 2, 3, 4]);
  });

  it("handles an empty marker list", () => {
    expect(toPlayerMarkers([])).toEqual([]);
  });
});

describe("toNextUp", () => {
  it("adds the title id the shared contract requires", () => {
    expect(toNextUp(NEXT_EPISODE, 7)).toEqual({
      titleId: 7,
      seasonNumber: 2,
      episodeNumber: 3,
      episodeId: 42,
      name: "The Long Way Down",
    });
  });

  it("keeps a nameless episode and refuses a film's absent offer", () => {
    expect(toNextUp({ ...NEXT_EPISODE, name: null }, 7)?.name).toBeNull();
    expect(toNextUp(null, 7)).toBeNull();
  });
});

describe("absolutePositionMs", () => {
  it("adds the transcode's offset but not the direct file's", () => {
    expect(absolutePositionMs("hls", 600_000, 0)).toBe(600_000);
    expect(absolutePositionMs("hls", 600_000, 12.5)).toBe(612_500);
    expect(absolutePositionMs("direct", 600_000, 12.5)).toBe(12_500);
  });

  it("never reports a negative or non-finite clock", () => {
    expect(absolutePositionMs("hls", -5_000, -2)).toBe(0);
    expect(absolutePositionMs("direct", 0, Number.NaN)).toBe(0);
    expect(absolutePositionMs("direct", 0, Number.POSITIVE_INFINITY)).toBe(0);
  });
});

describe("skipTarget", () => {
  it("names and targets the marker under the playhead", () => {
    expect(skipTarget(PLAYER_MARKERS, 10_000)).toEqual({
      label: "Skip Intro",
      targetMs: 70_000,
    });
    expect(skipTarget(PLAYER_MARKERS, 69_999)?.label).toBe("Skip Intro");
    expect(skipTarget(PLAYER_MARKERS, 70_000)?.label).toBe("Skip Recap");
    const previewOnly: readonly PlayerMarker[] = [
      { id: 9, kind: "preview", startMs: 1_000, endMs: 2_000, source: "chapters", confidence: 100 },
    ];
    expect(skipTarget(previewOnly, 1_000)).toEqual({ label: "Skip Preview", targetMs: 2_000 });
  });

  it("lets the earlier credits window win over a preview inside it", () => {
    expect(skipTarget(PLAYER_MARKERS, 5_950_000)).toBeNull();
  });

  it("offers nothing outside a window, and never for credits", () => {
    expect(skipTarget(PLAYER_MARKERS, 9_999)).toBeNull();
    expect(skipTarget(PLAYER_MARKERS, 80_000)).toBeNull();
    expect(skipTarget(PLAYER_MARKERS, 5_900_000)).toBeNull();
    expect(skipTarget(PLAYER_MARKERS, 6_500_000)).toBeNull();
  });

  it("refuses an open-ended marker and an unusable position", () => {
    const openIntro: readonly PlayerMarker[] = [
      { id: 1, kind: "intro", startMs: 1_000, endMs: null, source: "a", confidence: 80 },
    ];
    expect(skipTarget(openIntro, 2_000)).toBeNull();
    expect(skipTarget(PLAYER_MARKERS, Number.NaN)).toBeNull();
    expect(skipTarget([], 0)).toBeNull();
  });
});

describe("nextUpLabel", () => {
  it("names the episode and falls back to its coordinates", () => {
    const nextUp: NextUp = {
      titleId: 7,
      seasonNumber: 2,
      episodeNumber: 3,
      episodeId: 42,
      name: "The Long Way Down",
    };
    expect(nextUpLabel(nextUp)).toBe("S2 E3 · The Long Way Down");
    expect(nextUpLabel({ ...nextUp, name: "  " })).toBe("S2 E3");
    expect(nextUpLabel({ ...nextUp, name: null })).toBe("S2 E3");
  });
});

describe("hiddenPostPlay", () => {
  const nextUp: NextUp = {
    titleId: 7,
    seasonNumber: 2,
    episodeNumber: 3,
    episodeId: 42,
    name: "The Long Way Down",
  };

  it("seeds the next-up the credits and ended events reuse", () => {
    const state = hiddenPostPlay(nextUp);
    expect(state.phase).toBe("hidden");
    expect(state.nextUp).toEqual(nextUp);

    const credits = advance(state, { type: "credits" });
    expect(credits.state.phase).toBe("countdown");
    expect(credits.state.remainingMs).toBe(POST_PLAY_COUNTDOWN_MS);

    const ended = advance(state, { type: "ended" });
    expect(ended.state.phase).toBe("countdown");
    expect(ended.state.remainingMs).toBe(POST_PLAY_COUNTDOWN_MS);
  });

  it("ends a film on the panel", () => {
    const ended = advance(hiddenPostPlay(null), { type: "ended" });
    expect(ended.state.phase).toBe("ended");
    expect(ended.state.nextUp).toBeNull();
  });

  it("cannot start a follow-up after a cancellation", () => {
    const cancelled = advance(hiddenPostPlay(nextUp), { type: "credits" });
    const hidden = advance(cancelled.state, { type: "cancel" }).state;
    expect(hidden.nextUp).toBeNull();
    expect(advance(hidden, { type: "tick", elapsedMs: 99_999 }).command).toBeNull();
    expect(advance(hidden, { type: "start-next" }).command).toBeNull();
  });
});
