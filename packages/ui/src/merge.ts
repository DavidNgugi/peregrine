import { extendTailwindMerge } from "tailwind-merge";

/**
 * The design system's type scale as Tailwind class suffixes (docs/DESIGN_SYSTEM.md §3).
 *
 * It is written out here rather than imported from `@peregrine/theme`, which owns the same
 * vocabulary, because a package in this workspace is a leaf: the theme's token names are a
 * *test-time* input, and `merge.test.ts` asserts the two agree. Importing them at runtime
 * would have made `packages/ui` the first frontend package to depend on another package's
 * source, which its `rootDir` build cannot emit and which nothing else in the tree does.
 */
export const TYPE_SCALE_UTILITIES = [
  "display",
  "title-1",
  "title-2",
  "body",
  "body-strong",
  "caption",
  "micro",
] as const;

/**
 * `twMerge`, taught which `text-*` classes are sizes.
 *
 * The library decides which of two classes wins by putting them in groups, and it ships
 * knowing Tailwind's own scale. `text-caption` is not on that scale, so it is classified as
 * a *colour* — and a class that collides with one is dropped. The failure was silent and it
 * was not cosmetic: `Button variant="primary"` composes `text-text-on-accent` from its
 * variant and `text-caption` from its size, so the merge ate the token that carries §2.1's
 * contrast requirement, and a `size="sm"` control lost its font size whenever it also set a
 * colour.
 *
 * Every component in this package imports `twMerge` from this module and never from the
 * library, so the configuration cannot be half-applied.
 */
export const twMerge = extendTailwindMerge({
  extend: {
    classGroups: {
      "font-size": [{ text: [...TYPE_SCALE_UTILITIES] }],
    },
  },
});
