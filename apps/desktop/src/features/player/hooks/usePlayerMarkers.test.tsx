import type { PlaybackSessionDto } from "@peregrine/ipc-client";
import { describe, expect, it, vi } from "vitest";
import {
  isMarkerEventFor,
  markersFromSession,
  nextUpFromSession,
  startNextEpisode,
} from "./usePlayerMarkers.js";

/**
 * The pure half of the hook.
 *
 * The listener itself needs a DOM and Tauri's event bridge, neither of which a Node run has; the
 * rules worth pinning are the ones that decide what a listener would accept and how a session's
 * answer becomes the surface's values.
 */

type Marker = PlaybackSessionDto["markers"][number];

function session(overrides: Partial<PlaybackSessionDto> = {}): PlaybackSessionDto {
  return {
    sessionId: 1,
    titleId: 7,
    episodeId: 41,
    mediaFileId: 42,
    support: "webview",
    url: "http://127.0.0.1:51234/media/token",
    thumbnailUrl: null,
    subtitles: [],
    needsPoster: false,
    resumePositionMs: null,
    durationMs: 1_440_000,
    markers: [],
    nextEpisode: null,
    audioLanguage: "en",
    ...overrides,
  };
}

function marker(overrides: Partial<Marker> = {}): Marker {
  return {
    kind: "intro",
    startMs: 0,
    endMs: 90_000,
    source: "chapters",
    confidence: 90,
    ...overrides,
  };
}

describe("markersFromSession", () => {
  it("keeps the window, the source, and the confidence the core resolved", () => {
    const markers = markersFromSession([
      marker({ kind: "credits", startMs: 1_320_000, endMs: null, source: "introDb" }),
    ]);

    expect(markers).toEqual([
      {
        id: 1_320_000,
        kind: "credits",
        startMs: 1_320_000,
        endMs: null,
        source: "introDb",
        confidence: 90,
      },
    ]);
  });
});

describe("isMarkerEventFor", () => {
  it("accepts the subject this screen is playing", () => {
    expect(isMarkerEventFor({ titleId: 7, episodeId: 41, count: 2 }, 7, 41)).toBe(true);
    expect(isMarkerEventFor({ titleId: 7, episodeId: null }, 7, null)).toBe(true);
  });

  it("refuses another episode, another title, and a payload with no subject", () => {
    expect(isMarkerEventFor({ titleId: 7, episodeId: 42, count: 2 }, 7, 41)).toBe(false);
    expect(isMarkerEventFor({ titleId: 8, episodeId: 41, count: 2 }, 7, 41)).toBe(false);
    expect(isMarkerEventFor({ count: 2 }, 7, 41)).toBe(false);
    expect(isMarkerEventFor("player://markers", 7, 41)).toBe(false);
  });
});

describe("nextUpFromSession", () => {
  it("names the title the session belongs to, which the next-up DTO does not carry", () => {
    const next = nextUpFromSession(
      session({
        nextEpisode: {
          seasonNumber: 2,
          episodeNumber: 3,
          episodeId: 99,
          name: "The Long Way Down",
        },
      }),
    );

    expect(next).toEqual({
      titleId: 7,
      seasonNumber: 2,
      episodeNumber: 3,
      episodeId: 99,
      name: "The Long Way Down",
    });
  });

  it("offers nothing when the core found no follow-up, or there is no session", () => {
    expect(nextUpFromSession(session())).toBeNull();
    expect(nextUpFromSession(null)).toBeNull();
  });
});

describe("startNextEpisode", () => {
  it("opens the episode once, whether or not a prefetch warmed it", () => {
    const open = vi.fn(() => Promise.resolve());

    startNextEpisode(
      {
        titleId: 7,
        seasonNumber: 2,
        episodeNumber: 3,
        episodeId: 99,
        name: null,
      },
      open,
    );

    expect(open).toHaveBeenCalledTimes(1);
    expect(open).toHaveBeenCalledWith(7, 2, 3);
  });
});
