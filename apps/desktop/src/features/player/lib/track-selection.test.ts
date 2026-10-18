import type { SubtitleTrackDto } from "@peregrine/ipc-client";
import { describe, expect, it } from "vitest";

import { initialSubtitleTrack, subtitleTrackLabel } from "./track-selection.js";

/** One track as the core answers it, with only the field under test varying. */
function track(overrides: Partial<SubtitleTrackDto> = {}): SubtitleTrackDto {
  return {
    id: 11,
    language: "en",
    label: null,
    format: "vtt",
    forced: false,
    sdh: false,
    default: false,
    url: "http://127.0.0.1:5000/subtitle/token/11",
    ...overrides,
  };
}

describe("initialSubtitleTrack", () => {
  it("chooses the track the core marked default", () => {
    const tracks = [track({ id: 11 }), track({ id: 12, language: "fr", default: true })];

    expect(initialSubtitleTrack(tracks)).toBe(12);
  });

  it("chooses nothing when no track is default", () => {
    // A scan writes every sidecar with is_default false, so this is the ordinary case rather than an
    // edge: the menu opens on "Off" and the reader picks.
    expect(initialSubtitleTrack([track(), track({ id: 12 })])).toBeNull();
  });

  it("chooses nothing for a file with no sidecars", () => {
    expect(initialSubtitleTrack([])).toBeNull();
  });

  it("takes the first default when a database carries more than one", () => {
    // The schema permits it, so the choice has to be deterministic rather than left to whatever the
    // list happens to hold.
    expect(
      initialSubtitleTrack([track({ id: 11, default: true }), track({ id: 12, default: true })]),
    ).toBe(11);
  });
});

describe("subtitleTrackLabel", () => {
  it("prefers the core's label to the language tag", () => {
    expect(subtitleTrackLabel(track({ language: "fr", label: "Français" }))).toBe("Français · VTT");
  });

  it("names the language and the container when there is no label", () => {
    expect(subtitleTrackLabel(track({ language: "en", format: "srt" }))).toBe("en · SRT");
  });

  it("marks forced and SDH tracks so two English rows are distinguishable", () => {
    expect(subtitleTrackLabel(track({ forced: true, sdh: true, format: "ass" }))).toBe(
      "en · Forced · SDH · ASS",
    );
  });

  it("does not draw a blank label over the language", () => {
    expect(subtitleTrackLabel(track({ label: "   ", language: "de" }))).toBe("de · VTT");
  });
});
