import { describe, expect, it } from "vitest";

import { regionFromLocale } from "./region.js";

describe("regionFromLocale", () => {
  it("takes the region segment of a locale", () => {
    expect(regionFromLocale("en-US")).toBe("US");
    expect(regionFromLocale("pt-BR")).toBe("BR");
    expect(regionFromLocale("en_GB")).toBe("GB");
    expect(regionFromLocale("zh-Hant-TW")).toBe("TW");
  });

  it("falls back when the last segment is a script or absent", () => {
    expect(regionFromLocale("en")).toBe("US");
    expect(regionFromLocale("zh-Hant")).toBe("US");
    expect(regionFromLocale("")).toBe("US");
    expect(regionFromLocale("C")).toBe("US");
    expect(regionFromLocale("en-US", "KE")).toBe("US");
  });

  it("normalizes a lowercase region", () => {
    expect(regionFromLocale("en-us")).toBe("US");
  });
});
