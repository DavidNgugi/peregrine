import { describe, expect, it } from "vitest";
import {
  activeMarker,
  applyPlayerCommand,
  initialPlayerSnapshot,
  reducePlayerEvent,
} from "./player.js";
import type {
  NextUp,
  PlayerCapabilities,
  PlayerMarker,
  PlayerSnapshot,
  PlayerTrack,
} from "./player.js";

const CAPABILITIES: PlayerCapabilities = {
  pip: true,
  fullscreen: true,
  tracks: true,
  seek: true,
};

const NEXT_UP: NextUp = {
  titleId: 7,
  seasonNumber: 2,
  episodeNumber: 3,
  episodeId: 42,
  name: "The Long Way Down",
};

const AUDIO_TRACKS: readonly PlayerTrack[] = [
  { id: "a-en", label: "English 5.1", language: "en", selected: true },
  { id: "a-ja", label: "Japanese", language: "ja", selected: false },
];

const SUBTITLE_TRACKS: readonly PlayerTrack[] = [
  { id: "s-en", label: "English", language: "en", selected: false },
  { id: "s-en-sdh", label: "English SDH", language: "en", selected: false, sdh: true },
  { id: "s-fr", label: "French", language: "fr", selected: false, forced: true },
];

const MARKERS: readonly PlayerMarker[] = [
  {
    id: 1,
    kind: "intro",
    startMs: 10_000,
    endMs: 70_000,
    source: "chapters",
    confidence: 100,
  },
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

function playingSnapshot(overrides: Partial<PlayerSnapshot> = {}): PlayerSnapshot {
  return {
    ...initialPlayerSnapshot(),
    state: "playing",
    positionMs: 120_000,
    durationMs: 600_000,
    audioTracks: AUDIO_TRACKS,
    subtitleTracks: SUBTITLE_TRACKS,
    markers: MARKERS,
    nextUp: NEXT_UP,
    capabilities: CAPABILITIES,
    ...overrides,
  };
}

function deepFreeze<T>(value: T): T {
  if (value !== null && typeof value === "object") {
    for (const child of Object.values(value as Record<string, unknown>)) {
      deepFreeze(child);
    }
    Object.freeze(value);
  }
  return value;
}

describe("initialPlayerSnapshot", () => {
  it("describes a session before any media exists", () => {
    expect(initialPlayerSnapshot()).toEqual({
      state: "idle",
      positionMs: 0,
      durationMs: null,
      paused: false,
      buffering: false,
      volume: 1,
      rate: 1,
      audioTracks: [],
      subtitleTracks: [],
      markers: [],
      nextUp: null,
      error: null,
      capabilities: { pip: false, fullscreen: false, tracks: false, seek: false },
    });
  });

  it("returns a fresh value so one session cannot write another's snapshot", () => {
    expect(initialPlayerSnapshot()).not.toBe(initialPlayerSnapshot());
  });
});

describe("applyPlayerCommand", () => {
  it("derives the next snapshot from the state and command alone", () => {
    const state = playingSnapshot();
    const first = applyPlayerCommand(state, { type: "pause" });
    const second = applyPlayerCommand(structuredClone(state), { type: "pause" });
    expect(first).toEqual(second);
  });

  it("never mutates the state it was given", () => {
    const state = deepFreeze(playingSnapshot({ error: "old failure" }));
    const before = structuredClone(state);

    applyPlayerCommand(state, { type: "seek", positionMs: 300_000 });
    applyPlayerCommand(state, { type: "selectAudio", trackId: "a-ja" });
    applyPlayerCommand(state, { type: "selectSubtitle", trackId: null });
    applyPlayerCommand(state, { type: "setVolume", volume: 0.25 });
    applyPlayerCommand(state, { type: "setRate", rate: 2 });
    applyPlayerCommand(state, { type: "stop" });

    expect(state).toEqual(before);
  });

  it("loads at zero by default and clears everything the previous media held", () => {
    const state = playingSnapshot({ state: "failed", error: "decoder died" });
    const next = applyPlayerCommand(state, { type: "load", url: "https://media.example/one.mkv" });

    expect(next).toEqual({
      ...initialPlayerSnapshot(),
      state: "loading",
      capabilities: CAPABILITIES,
    });
  });

  it("resumes at a validated start position", () => {
    const state = playingSnapshot();
    expect(
      applyPlayerCommand(state, {
        type: "load",
        url: "https://media.example/one.mkv",
        startPositionMs: 42_000,
      }).positionMs,
    ).toBe(42_000);
    expect(
      applyPlayerCommand(state, {
        type: "load",
        url: "https://media.example/one.mkv",
        startPositionMs: -5,
      }).positionMs,
    ).toBe(0);
    expect(
      applyPlayerCommand(state, {
        type: "load",
        url: "https://media.example/one.mkv",
        startPositionMs: Number.NaN,
      }).positionMs,
    ).toBe(0);
  });

  it("refuses a blank source instead of opening nothing", () => {
    const state = playingSnapshot();
    expect(applyPlayerCommand(state, { type: "load", url: "   " })).toBe(state);
  });

  it("stops to idle while keeping the engine's preferences", () => {
    const state = playingSnapshot({ volume: 0.4, rate: 1.25 });
    const next = applyPlayerCommand(state, { type: "stop" });

    expect(next).toEqual({
      ...initialPlayerSnapshot(),
      volume: 0.4,
      rate: 1.25,
      capabilities: CAPABILITIES,
    });
  });

  it("retries only a failed session, from the position it held", () => {
    const failed = playingSnapshot({ state: "failed", error: "decoder died" });
    const retried = applyPlayerCommand(failed, { type: "retry" });

    expect(retried.state).toBe("loading");
    expect(retried.error).toBeNull();
    expect(retried.positionMs).toBe(failed.positionMs);
    expect(retried.durationMs).toBe(failed.durationMs);
    expect(retried.paused).toBe(false);

    for (const state of [initialPlayerSnapshot(), playingSnapshot({ state: "ended" })]) {
      expect(applyPlayerCommand(state, { type: "retry" })).toBe(state);
    }
  });

  it("clamps seek to the media, and to zero when the duration is unknown", () => {
    const state = playingSnapshot();

    expect(applyPlayerCommand(state, { type: "seek", positionMs: -1 }).positionMs).toBe(0);
    expect(applyPlayerCommand(state, { type: "seek", positionMs: 0 }).positionMs).toBe(0);
    expect(applyPlayerCommand(state, { type: "seek", positionMs: 600_000 }).positionMs).toBe(
      600_000,
    );
    expect(applyPlayerCommand(state, { type: "seek", positionMs: 600_001 }).positionMs).toBe(
      600_000,
    );
    expect(applyPlayerCommand(state, { type: "seek", positionMs: Number.NaN })).toBe(state);

    const live = playingSnapshot({ durationMs: null });
    expect(applyPlayerCommand(live, { type: "seek", positionMs: 9_999_999 }).positionMs).toBe(
      9_999_999,
    );

    const malformed = playingSnapshot({ durationMs: -5 });
    expect(applyPlayerCommand(malformed, { type: "seek", positionMs: 100 }).positionMs).toBe(100);
  });

  it("refuses transport commands outside a live session", () => {
    for (const state of [
      initialPlayerSnapshot(),
      playingSnapshot({ state: "ended" }),
      playingSnapshot({ state: "failed", error: "boom" }),
    ]) {
      expect(applyPlayerCommand(state, { type: "play" })).toBe(state);
      expect(applyPlayerCommand(state, { type: "pause" })).toBe(state);
      expect(applyPlayerCommand(state, { type: "seek", positionMs: 1 })).toBe(state);
      expect(applyPlayerCommand(state, { type: "selectAudio", trackId: "a-en" })).toBe(state);
      expect(applyPlayerCommand(state, { type: "selectSubtitle", trackId: "s-en" })).toBe(state);
    }
  });

  it("clamps rate to the supported range", () => {
    const state = playingSnapshot();
    expect(applyPlayerCommand(state, { type: "setRate", rate: 0.1 }).rate).toBe(0.25);
    expect(applyPlayerCommand(state, { type: "setRate", rate: 0.25 }).rate).toBe(0.25);
    expect(applyPlayerCommand(state, { type: "setRate", rate: 4 }).rate).toBe(4);
    expect(applyPlayerCommand(state, { type: "setRate", rate: 10 }).rate).toBe(4);
    expect(applyPlayerCommand(state, { type: "setRate", rate: Number.NaN })).toBe(state);
    expect(applyPlayerCommand(state, { type: "setRate", rate: Number.POSITIVE_INFINITY })).toBe(
      state,
    );
  });

  it("clamps volume to the unit interval", () => {
    const state = playingSnapshot();
    expect(applyPlayerCommand(state, { type: "setVolume", volume: -1 }).volume).toBe(0);
    expect(applyPlayerCommand(state, { type: "setVolume", volume: 0.35 }).volume).toBe(0.35);
    expect(applyPlayerCommand(state, { type: "setVolume", volume: 0 }).volume).toBe(0);
    expect(applyPlayerCommand(state, { type: "setVolume", volume: 1 }).volume).toBe(1);
    expect(applyPlayerCommand(state, { type: "setVolume", volume: 2 }).volume).toBe(1);
    expect(applyPlayerCommand(state, { type: "setVolume", volume: Number.NaN })).toBe(state);
  });

  it("moves audio selection to exactly one existing track", () => {
    const state = playingSnapshot();
    const next = applyPlayerCommand(state, { type: "selectAudio", trackId: "a-ja" });
    expect(next.audioTracks.map((track) => [track.id, track.selected])).toEqual([
      ["a-en", false],
      ["a-ja", true],
    ]);
    expect(state.audioTracks.map((track) => track.selected)).toEqual([true, false]);
    expect(applyPlayerCommand(state, { type: "selectAudio", trackId: "missing" })).toBe(state);
  });

  it("turns subtitles on one track, or off entirely", () => {
    const state = playingSnapshot();
    const selected = applyPlayerCommand(state, { type: "selectSubtitle", trackId: "s-en-sdh" });
    expect(selected.subtitleTracks.map((track) => track.selected)).toEqual([false, true, false]);

    const off = applyPlayerCommand(selected, { type: "selectSubtitle", trackId: null });
    expect(off.subtitleTracks.every((track) => !track.selected)).toBe(true);

    expect(applyPlayerCommand(state, { type: "selectSubtitle", trackId: "missing" })).toBe(state);
  });

  it("moves between the live states", () => {
    const paused = applyPlayerCommand(playingSnapshot(), { type: "pause" });
    expect(paused.state).toBe("paused");
    expect(paused.paused).toBe(true);
    expect(paused.buffering).toBe(false);

    const resumed = applyPlayerCommand(paused, { type: "play" });
    expect(resumed.state).toBe("playing");
    expect(resumed.paused).toBe(false);
  });
});

describe("reducePlayerEvent", () => {
  it("moves to loading and forgets the previous error", () => {
    const state = playingSnapshot({ state: "failed", error: "decoder died" });
    const next = reducePlayerEvent(state, { type: "loading" });
    expect(next.state).toBe("loading");
    expect(next.error).toBeNull();
    expect(next.paused).toBe(false);
    expect(next.buffering).toBe(false);
  });

  it("fills the timeline, tracks, markers, and next-up from a loaded container", () => {
    const loading = applyPlayerCommand(initialPlayerSnapshot(), {
      type: "load",
      url: "https://media.example/one.mkv",
    });
    const next = reducePlayerEvent(loading, {
      type: "loaded",
      durationMs: 600_000,
      audioTracks: AUDIO_TRACKS,
      subtitleTracks: SUBTITLE_TRACKS,
      markers: MARKERS,
      nextUp: NEXT_UP,
    });

    expect(next.state).toBe("paused");
    expect(next.paused).toBe(true);
    expect(next.durationMs).toBe(600_000);
    expect(next.audioTracks).toEqual(AUDIO_TRACKS);
    expect(next.subtitleTracks).toEqual(SUBTITLE_TRACKS);
    expect(next.markers).toEqual(MARKERS);
    expect(next.nextUp).toEqual(NEXT_UP);
  });

  it("does not pause a session that was already asked to play", () => {
    const loading = applyPlayerCommand(initialPlayerSnapshot(), {
      type: "load",
      url: "https://media.example/one.mkv",
    });
    const playing = applyPlayerCommand(loading, { type: "play" });
    const next = reducePlayerEvent(playing, {
      type: "loaded",
      durationMs: 600_000,
      audioTracks: [],
      subtitleTracks: [],
      markers: [],
      nextUp: null,
    });
    expect(next.state).toBe("playing");
  });

  it("reports an unknown duration as null", () => {
    const state = playingSnapshot();
    for (const durationMs of [Number.NaN, Number.POSITIVE_INFINITY, -1]) {
      const next = reducePlayerEvent(state, {
        type: "loaded",
        durationMs,
        audioTracks: [],
        subtitleTracks: [],
        markers: [],
        nextUp: null,
      });
      expect(next.durationMs).toBeNull();
    }
  });

  it("ignores a loaded event for a session that does not exist", () => {
    const state = initialPlayerSnapshot();
    expect(
      reducePlayerEvent(state, {
        type: "loaded",
        durationMs: 1,
        audioTracks: [],
        subtitleTracks: [],
        markers: [],
        nextUp: null,
      }),
    ).toBe(state);
  });

  it("moves the position without the state changing, and clamps it to the duration", () => {
    const state = playingSnapshot();
    const moved = reducePlayerEvent(state, { type: "position", positionMs: 200_000 });
    expect(moved.positionMs).toBe(200_000);
    expect(moved.state).toBe("playing");
    expect(reducePlayerEvent(state, { type: "position", positionMs: 700_000 }).positionMs).toBe(
      600_000,
    );
    expect(reducePlayerEvent(state, { type: "position", positionMs: -10 }).positionMs).toBe(0);
    expect(reducePlayerEvent(state, { type: "position", positionMs: Number.NaN })).toBe(state);
  });

  it("ends at the duration and then absorbs later transport events", () => {
    const ended = reducePlayerEvent(playingSnapshot(), { type: "ended" });
    expect(ended.state).toBe("ended");
    expect(ended.positionMs).toBe(600_000);
    expect(ended.paused).toBe(false);
    expect(ended.buffering).toBe(false);

    expect(reducePlayerEvent(ended, { type: "position", positionMs: 1 })).toBe(ended);
    expect(reducePlayerEvent(ended, { type: "playing" })).toBe(ended);
    expect(reducePlayerEvent(ended, { type: "paused" })).toBe(ended);
    expect(reducePlayerEvent(ended, { type: "ended" })).toBe(ended);
  });

  it("carries a useful failure message and ignores a duplicate failure", () => {
    const failed = reducePlayerEvent(playingSnapshot(), {
      type: "failed",
      error: "  decoder died  ",
    });
    expect(failed.state).toBe("failed");
    expect(failed.error).toBe("decoder died");
    expect(failed.paused).toBe(false);
    expect(failed.buffering).toBe(false);

    const blank = reducePlayerEvent(playingSnapshot(), { type: "failed", error: "   " });
    expect(blank.error).toBe("Playback failed.");

    expect(reducePlayerEvent(failed, { type: "failed", error: "second failure" })).toBe(failed);
  });

  it("keeps next-up and marker metadata that arrives late", () => {
    const ended = reducePlayerEvent(playingSnapshot(), { type: "ended" });
    const withNext = reducePlayerEvent(ended, { type: "nextUp", nextUp: NEXT_UP });
    expect(withNext.nextUp).toEqual(NEXT_UP);

    const withMarkers = reducePlayerEvent(withNext, { type: "markers", markers: MARKERS });
    expect(withMarkers.markers).toEqual(MARKERS);
    expect(withMarkers.state).toBe("ended");
  });

  it("replaces tracks without keeping a reference to the engine's arrays", () => {
    const state = playingSnapshot();
    const next = reducePlayerEvent(state, {
      type: "tracks",
      audioTracks: AUDIO_TRACKS,
      subtitleTracks: SUBTITLE_TRACKS,
    });
    expect(next.audioTracks).toEqual(AUDIO_TRACKS);
    expect(next.audioTracks).not.toBe(AUDIO_TRACKS);
    expect(next.subtitleTracks).not.toBe(SUBTITLE_TRACKS);
  });

  it("clamps volume and rate reports", () => {
    const state = playingSnapshot();
    expect(reducePlayerEvent(state, { type: "volume", volume: -1 }).volume).toBe(0);
    expect(reducePlayerEvent(state, { type: "volume", volume: 2 }).volume).toBe(1);
    expect(reducePlayerEvent(state, { type: "rate", rate: 0.1 }).rate).toBe(0.25);
    expect(reducePlayerEvent(state, { type: "rate", rate: 99 }).rate).toBe(4);
    expect(reducePlayerEvent(state, { type: "volume", volume: Number.NaN })).toBe(state);
    expect(reducePlayerEvent(state, { type: "rate", rate: Number.NaN })).toBe(state);
  });

  it("reports engine capabilities even before a load", () => {
    const state = initialPlayerSnapshot();
    const next = reducePlayerEvent(state, { type: "capabilities", capabilities: CAPABILITIES });
    expect(next.capabilities).toEqual(CAPABILITIES);
    expect(next.state).toBe("idle");
  });

  it("refuses events for an idle session", () => {
    const state = initialPlayerSnapshot();
    for (const event of [
      { type: "playing" as const },
      { type: "paused" as const },
      { type: "buffering" as const },
      { type: "position" as const, positionMs: 100 },
      { type: "ended" as const },
      { type: "failed" as const, error: "boom" },
      { type: "nextUp" as const, nextUp: NEXT_UP },
      { type: "tracks" as const, audioTracks: AUDIO_TRACKS, subtitleTracks: SUBTITLE_TRACKS },
      { type: "markers" as const, markers: MARKERS },
    ]) {
      expect(reducePlayerEvent(state, event)).toBe(state);
    }
  });
});

describe("activeMarker", () => {
  it("treats a start as inside and an end as outside", () => {
    expect(activeMarker(MARKERS, 9_999)).toBeNull();
    expect(activeMarker(MARKERS, 10_000)?.id).toBe(1);
    expect(activeMarker(MARKERS, 69_999)?.id).toBe(1);
    expect(activeMarker(MARKERS, 70_000)?.id).toBe(2);
    expect(activeMarker(MARKERS, 79_999)?.id).toBe(2);
    expect(activeMarker(MARKERS, 80_000)).toBeNull();
  });

  it("runs an open-ended credits marker to the end of the media", () => {
    expect(activeMarker(MARKERS, 5_899_999)).toBeNull();
    expect(activeMarker(MARKERS, 5_900_000)?.kind).toBe("credits");
    expect(activeMarker(MARKERS, 10_000_000)?.kind).toBe("credits");
  });

  it("prefers the earliest start when windows overlap", () => {
    expect(activeMarker(MARKERS, 5_950_000)?.id).toBe(3);
    expect(activeMarker(MARKERS, 5_950_000)?.kind).toBe("credits");

    const overlapping: readonly PlayerMarker[] = [
      { id: 10, kind: "recap", startMs: 20_000, endMs: 30_000, source: "a", confidence: 80 },
      { id: 11, kind: "intro", startMs: 10_000, endMs: 25_000, source: "b", confidence: 80 },
    ];
    expect(activeMarker(overlapping, 22_000)?.id).toBe(11);
  });

  it("keeps the first marker when two windows start together", () => {
    const tied: readonly PlayerMarker[] = [
      { id: 20, kind: "intro", startMs: 10_000, endMs: 20_000, source: "a", confidence: 80 },
      { id: 21, kind: "recap", startMs: 10_000, endMs: 15_000, source: "b", confidence: 80 },
    ];
    expect(activeMarker(tied, 12_000)?.id).toBe(20);
  });

  it("returns null for no markers or an unusable position", () => {
    expect(activeMarker([], 0)).toBeNull();
    expect(activeMarker(MARKERS, Number.NaN)).toBeNull();
    expect(activeMarker(MARKERS, Number.POSITIVE_INFINITY)).toBeNull();
  });

  it("ignores a collapsed or malformed window", () => {
    const malformed: readonly PlayerMarker[] = [
      { id: 30, kind: "intro", startMs: 1_000, endMs: 1_000, source: "a", confidence: 80 },
      { id: 31, kind: "intro", startMs: Number.NaN, endMs: 2_000, source: "a", confidence: 80 },
      { id: 32, kind: "intro", startMs: 3_000, endMs: Number.NaN, source: "a", confidence: 80 },
    ];
    expect(activeMarker(malformed, 1_000)).toBeNull();
    expect(activeMarker(malformed, 1_500)).toBeNull();
    expect(activeMarker(malformed, 3_500)).toBeNull();
  });
});
