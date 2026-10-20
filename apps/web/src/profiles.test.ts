import { describe, expect, it } from "vitest";

import {
  DEFAULT_BOOK,
  activeProfile,
  initialsFor,
  parseProfileBook,
  selecting,
  withProfile,
} from "./profiles.js";

describe("parseProfileBook", () => {
  it("starts from You when storage is empty or broken", () => {
    expect(parseProfileBook(null)).toEqual(DEFAULT_BOOK);
    expect(parseProfileBook("not json")).toEqual(DEFAULT_BOOK);
    expect(parseProfileBook(JSON.stringify({ profiles: [] }))).toEqual(DEFAULT_BOOK);
  });

  it("keeps a valid list and falls back when the active id is gone", () => {
    const stored = {
      profiles: [
        { id: "a", name: "Ada", hue: 20 },
        { id: "b", name: "Ben", hue: 40 },
      ],
      activeId: "gone",
    };
    const book = parseProfileBook(JSON.stringify(stored));
    expect(book.profiles).toHaveLength(2);
    expect(book.activeId).toBe("a");
  });
});

describe("profile actions", () => {
  it("adds a profile and selects it", () => {
    const next = withProfile(DEFAULT_BOOK, "  Ada  ");
    expect(next.profiles.map((profile) => profile.name)).toEqual(["You", "Ada"]);
    expect(activeProfile(next).name).toBe("Ada");
  });

  it("ignores a blank name", () => {
    expect(withProfile(DEFAULT_BOOK, "   ")).toBe(DEFAULT_BOOK);
  });

  it("selects only a profile that exists", () => {
    const withAda = withProfile(DEFAULT_BOOK, "Ada");
    const ada = withAda.profiles.find((profile) => profile.name === "Ada");
    if (ada === undefined) throw new Error("the fixture must hold Ada");
    expect(activeProfile(selecting(withAda, ada.id)).name).toBe("Ada");
    expect(selecting(withAda, "missing")).toBe(withAda);
  });
});

describe("initialsFor", () => {
  it("uses the first and last word, at most two letters", () => {
    expect(initialsFor("Ada Lovelace")).toBe("AL");
    expect(initialsFor("You")).toBe("Y");
    expect(initialsFor("  ")).toBe("?");
  });
});
