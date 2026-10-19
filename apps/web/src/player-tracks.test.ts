import { describe, expect, it } from "vitest";

import type { NextEpisode, SubtitleTrack } from "./api.js";
import {
  audioOptionIndex,
  initialSubtitleIndex,
  nextEpisodeRequest,
  subtitleLabel,
  totalDurationMs,
} from "./player-tracks.js";

function subtitle(overrides: Partial<SubtitleTrack> = {}): SubtitleTrack {
  return {
    id: 1,
    language: "en",
    label: "English",
    format: "vtt",
    forced: false,
    sdh: false,
    default: false,
    url: "/api/subtitles/1.vtt",
    ...overrides,
  };
}

describe("initialSubtitleIndex", () => {
  it("chooses the track the core marked default", () => {
    const tracks = [subtitle({ id: 4 }), subtitle({ id: 9, default: true })];
    expect(initialSubtitleIndex(tracks)).toBe(1);
  });

  it("is Off when a ticket has tracks but none is default", () => {
    expect(initialSubtitleIndex([subtitle({ id: 4 })])).toBeNull();
  });

  it("is Off when a ticket has no tracks", () => {
    expect(initialSubtitleIndex([])).toBeNull();
  });
});

describe("subtitleLabel", () => {
  it("falls back to the language when the library gave no label", () => {
    expect(subtitleLabel(subtitle({ label: null }))).toBe("en");
    expect(subtitleLabel(subtitle({ label: "   " }))).toBe("en");
  });

  it("marks forced and SDH tracks so two English rows are distinguishable", () => {
    expect(subtitleLabel(subtitle({ forced: true }))).toBe("English (forced)");
    expect(subtitleLabel(subtitle({ sdh: true }))).toBe("English (SDH)");
    expect(subtitleLabel(subtitle({ forced: true, sdh: true }))).toBe("English (forced, SDH)");
  });
});

describe("totalDurationMs", () => {
  it("keeps the ticket's full duration instead of the HLS playlist's encoded length", () => {
    expect(totalDurationMs(8_891_077, 0, 11)).toBe(8_891_077);
    expect(totalDurationMs(8_891_077, 3_600_000, 11)).toBe(8_891_077);
  });

  it("falls back to the element only when the ticket has no duration", () => {
    expect(totalDurationMs(null, 0, 11)).toBe(11_000);
    expect(totalDurationMs(null, 3_600_000, 11)).toBe(3_611_000);
  });

  it("has no total when neither source knows one", () => {
    expect(totalDurationMs(null, 0, Number.NaN)).toBeNull();
    expect(totalDurationMs(null, 0, Infinity)).toBeNull();
    expect(totalDurationMs(null, 0, 0)).toBeNull();
  });
});

describe("nextEpisodeRequest", () => {
  it("asks for the following episode from the beginning", () => {
    const next: NextEpisode = {
      seasonNumber: 3,
      episodeNumber: 7,
      episodeId: 42,
      name: "The Long Way",
    };
    expect(nextEpisodeRequest(next)).toEqual({
      positionMs: 0,
      seasonNumber: 3,
      episodeNumber: 7,
    });
  });

  it("has nothing to ask for when a film or the last episode ends", () => {
    expect(nextEpisodeRequest(null)).toBeNull();
  });
});

describe("audioOptionIndex", () => {
  it("selects the rendition matching the host's audio language, ignoring case", () => {
    const options = [{ language: "en" }, { language: "fr" }];
    expect(audioOptionIndex(options, "fr")).toBe(1);
    expect(audioOptionIndex(options, "FR")).toBe(1);
  });

  it("falls back to the first rendition when nothing matches", () => {
    expect(audioOptionIndex([{ language: "en" }, { language: "fr" }], "de")).toBe(0);
    expect(audioOptionIndex([{ language: null }], "en")).toBe(0);
  });

  it("is null when the browser exposed no rendition at all", () => {
    expect(audioOptionIndex([], "en")).toBeNull();
  });
});
