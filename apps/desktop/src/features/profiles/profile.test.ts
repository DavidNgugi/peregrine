import { describe, expect, it } from "vitest";

import {
  DEFAULT_BOOK,
  activeProfile,
  hueFor,
  initialsFor,
  loadProfileBook,
  newProfile,
} from "./profile.js";

describe("profiles", () => {
  it("derives a stable hue and initials from a name", () => {
    expect(hueFor("You")).toBe(hueFor("You"));
    expect(hueFor("You")).toBeGreaterThanOrEqual(0);
    expect(hueFor("You")).toBeLessThan(360);
    expect(initialsFor("Ada Lovelace")).toBe("AL");
    expect(initialsFor("You")).toBe("Y");
    expect(initialsFor("   ")).toBe("?");
  });

  it("creates a profile whose id is stable and whose hue follows the name", () => {
    const profile = newProfile("Ada");
    expect(profile.name).toBe("Ada");
    expect(profile.id.length).toBeGreaterThan(0);
    expect(profile.hue).toBe(hueFor("Ada"));
  });

  it("falls back to the default book without a storage", () => {
    expect(loadProfileBook()).toEqual(DEFAULT_BOOK);
    expect(activeProfile(DEFAULT_BOOK)).toEqual(DEFAULT_BOOK.profiles[0]);
  });
});
