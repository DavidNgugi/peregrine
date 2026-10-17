import { describe, expect, it } from "vitest";

import { DEFAULT_PROFILE, hueFor, initialsFor, loadProfile } from "./profile.js";

describe("profile", () => {
  it("derives a stable hue and initials from a name", () => {
    expect(hueFor("You")).toBe(hueFor("You"));
    expect(hueFor("You")).toBeGreaterThanOrEqual(0);
    expect(hueFor("You")).toBeLessThan(360);
    expect(initialsFor("Ada Lovelace")).toBe("AL");
    expect(initialsFor("You")).toBe("Y");
    expect(initialsFor("   ")).toBe("?");
  });

  it("falls back to the default without a storage", () => {
    expect(loadProfile()).toEqual(DEFAULT_PROFILE);
  });
});
