import type { SeasonDto } from "@peregrine/ipc-client";
import { createI18n } from "@peregrine/i18n";
import { describe, expect, it } from "vitest";

import { defaultSeason, episodeAvailability } from "./seasons.js";

function season(overrides: Partial<SeasonDto> = {}): SeasonDto {
  return {
    seasonNumber: 1,
    name: null,
    episodeCount: null,
    airDate: null,
    poster: null,
    ...overrides,
  };
}

describe("defaultSeason", () => {
  it("opens on the earliest numbered season rather than on the specials", () => {
    // **The whole reason this is a function.** The read orders by number, so season zero comes first —
    // and a show that has one would open on its bonus disc instead of on its first season.
    const seasons = [
      season({ seasonNumber: 0, name: "Specials" }),
      season({ seasonNumber: 1 }),
      season({ seasonNumber: 2 }),
    ];

    expect(defaultSeason(seasons)?.seasonNumber).toBe(1);
  });

  it("falls back to the specials when they are all a show has", () => {
    // Nothing numbered is not nothing to show: a show whose only listed season is its specials opens on
    // them rather than on an empty page.
    expect(defaultSeason([season({ seasonNumber: 0, name: "Specials" })])?.seasonNumber).toBe(0);
  });

  it("answers nothing for a show no provider has described", () => {
    // `null` rather than a fabricated season one: the screen draws no section at all, which is the
    // state a show nobody has fetched arrives in.
    expect(defaultSeason([])).toBeNull();
  });
});

describe("episodeAvailability", () => {
  it("calls an episode with every file present available", () => {
    expect(episodeAvailability({ items: 1, files: 1, missingFiles: 0 })).toBe("available");
  });

  it("calls an episode with some files gone partial, and one with none left missing", () => {
    // The distinction a reader needs: "partly here" is a season that plays most of the way, and
    // "files are gone" is one that does not play at all. Reporting the second as the first would be the
    // wrong thing to say about an episode nothing can play.
    expect(episodeAvailability({ items: 2, files: 3, missingFiles: 1 })).toBe("partial");
    expect(episodeAvailability({ items: 2, files: 3, missingFiles: 3 })).toBe("missing");
  });

  it("calls an episode the library holds nothing of wanted", () => {
    // Not `available`, which would be a lie, and not silence either: an episode with no file at all is
    // the difference between a season that is short and one that was never acquired.
    expect(episodeAvailability({ items: 0, files: 0, missingFiles: 0 })).toBe("wanted");
  });
});

describe("the season chip's own reads", () => {
  it("has a word for every season number and every availability it can report", () => {
    // The keys a chip draws, asserted against a real catalogue so a missing translation is a failing
    // test rather than a chip reading `title.season.number` on screen — which is exactly what happened
    // the first time this copy was nested one level too deep.
    const i18n = createI18n("en");

    expect(i18n.t("title.season.number", { number: 4 })).toBe("Season 4");
    expect(i18n.t("title.season.specials")).toBe("Specials");
    expect(i18n.t("title.season.label")).toBe("Seasons");
    for (const state of ["available", "partial", "missing", "wanted"]) {
      expect(i18n.t(`title.episode.availability.${state}`)).not.toContain("title.");
    }
    expect(i18n.t("title.episode.still.none")).not.toContain("title.");
    expect(i18n.t("title.episodes.none.title")).not.toContain("title.");
  });
});
