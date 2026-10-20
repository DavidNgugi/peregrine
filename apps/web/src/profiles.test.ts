import { describe, expect, it } from "vitest";

import { avatarSrc, hueFor, initialsFor } from "./profiles.js";

describe("initialsFor", () => {
  it("uses the first and last word, at most two letters", () => {
    expect(initialsFor("Ada Lovelace")).toBe("AL");
    expect(initialsFor("You")).toBe("Y");
    expect(initialsFor("  ")).toBe("?");
  });
});

describe("avatarSrc", () => {
  it("accepts only an image data URL", () => {
    expect(avatarSrc("data:image/png;base64,AA")).toBe("data:image/png;base64,AA");
    expect(avatarSrc(null)).toBeNull();
    expect(avatarSrc("https://example.test/a.png")).toBeNull();
  });
});

describe("hueFor", () => {
  it("is stable and inside the wheel", () => {
    expect(hueFor("Ada")).toBe(hueFor("Ada"));
    expect(hueFor("Ada")).toBeGreaterThanOrEqual(0);
    expect(hueFor("Ada")).toBeLessThan(360);
  });
});
