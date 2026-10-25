import { describe, expect, it } from "vitest";

import { languageMatches, preferredTrackIndex, type AudioTrackLike } from "./audio-track.js";

const tracks = (...languages: string[]): AudioTrackLike[] =>
  languages.map((language) => ({ language, enabled: false }));

describe("languageMatches", () => {
  it("accepts the same tag in either case", () => {
    expect(languageMatches("en", "EN")).toBe(true);
    expect(languageMatches("eng", "eng")).toBe(true);
  });

  it("accepts a two-letter preference against the container's three-letter tag", () => {
    expect(languageMatches("en", "eng")).toBe(true);
    expect(languageMatches("it", "ita")).toBe(true);
    expect(languageMatches("pt", "por")).toBe(true);
  });

  it("accepts a regional track for a language-wide preference and the reverse", () => {
    expect(languageMatches("en", "en-GB")).toBe(true);
    expect(languageMatches("en-GB", "en")).toBe(true);
  });

  it("does not match a different language or an untagged track", () => {
    expect(languageMatches("en", "ita")).toBe(false);
    expect(languageMatches("en", "und")).toBe(false);
    expect(languageMatches("", "eng")).toBe(false);
  });
});

describe("preferredTrackIndex", () => {
  it("finds the English track after an Italian default", () => {
    const list = tracks("ita", "eng");
    expect(preferredTrackIndex(list, "en")).toBe(1);
  });

  it("leaves the engine alone when nothing matches", () => {
    expect(preferredTrackIndex(tracks("ita", "fra"), "en")).toBeNull();
  });

  it("prefers the first match when a file carries two English tracks", () => {
    expect(preferredTrackIndex(tracks("eng", "en-GB"), "en")).toBe(0);
  });
});
