import { TYPOGRAPHY_SIZE_UTILITIES } from "@peregrine/theme";
import { describe, expect, it } from "vitest";
import { twMerge, TYPE_SCALE_UTILITIES } from "./merge.js";

describe("TYPE_SCALE_UTILITIES", () => {
  it("names exactly the type scale the theme ships", () => {
    // The list is written out in `merge.ts` because `packages/ui` may not import another
    // package at runtime — it would break that package's `rootDir` build, and no frontend
    // package depends on another today. This assertion is what keeps the copy honest: a
    // token added, renamed, or removed in `@peregrine/theme` fails here instead of
    // silently losing a class from every component that uses it.
    expect([...TYPE_SCALE_UTILITIES]).toEqual([...TYPOGRAPHY_SIZE_UTILITIES]);
  });
});

describe("twMerge", () => {
  it("keeps a size and a colour from the design system's scale together", () => {
    // Without the scale registered, `text-caption` is read as a colour class and dropped
    // in favour of `text-text-secondary` — a silent loss of the caption size.
    expect(twMerge("text-caption text-text-secondary")).toBe("text-caption text-text-secondary");
  });

  it("keeps the colour a variant contributes when a size is merged after it", () => {
    // The exact composition `Button`'s cva produces, in the order cva produces it: the
    // variant's colour first, the size's font size second. The colour must survive.
    const merged = twMerge("inline-flex bg-accent text-text-on-accent", "h-7 px-2 text-caption");

    expect(merged).toContain("text-text-on-accent");
    expect(merged).toContain("text-caption");
  });

  it("still lets a call site win a real conflict", () => {
    // The reason the library is here at all, and the property `Button`'s own test names:
    // two classes in the same group must still collapse to the last one.
    expect(twMerge("text-text-primary", "text-text-secondary")).toBe("text-text-secondary");
    expect(twMerge("bg-accent", "bg-surface-2")).toBe("bg-surface-2");
  });

  it("knows every size in the scale, not just the one that broke", () => {
    for (const size of TYPE_SCALE_UTILITIES) {
      expect(twMerge(`text-${size} text-text-secondary`), size).toBe(
        `text-${size} text-text-secondary`,
      );
    }
  });
});
