import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { Skeleton } from "./Skeleton.js";

/**
 * The element's classes, as the component wrote them.
 *
 * Read through the attribute and not off the whole markup, because a substring test over the markup
 * asks about the wrong thing: `overflow-hidden` contains the characters `w-` and is not a width.
 * `React` escapes the apostrophes of an arbitrary value — `after:content-['']` arrives as
 * `after:content-[&#x27;&#x27;]` — so the entity is put back before a class is compared with the
 * spelling in `Skeleton.tsx`.
 */
function classesOf(markup: string): string[] {
  const raw = (/class="([^"]*)"/u.exec(markup)?.[1] ?? "").replaceAll("&#x27;", "'");
  return raw.split(/\s+/u).filter((name) => name !== "");
}

/** A utility's own name, without the variant that says when it applies. */
function utilityOf(name: string): string {
  return name.slice(name.lastIndexOf(":") + 1);
}

/** The utilities that fix a box's size, which a skeleton never states for itself. */
const SHAPE_UTILITY = /^(?:min-|max-)?[wh]-|^aspect-/u;

describe("Skeleton", () => {
  it("carries no shape of its own, so the caller can state what is coming", () => {
    // §11 rule 2's "skeleton with real geometry" is only real if the shape is the content's: a
    // placeholder with a built-in size would be a box the arriving content does not fit, which is the
    // layout shift the rule exists to prevent. A variant such as `after:` only says when a utility
    // applies, so it is stripped before the utility itself is judged.
    const shapes = classesOf(renderToStaticMarkup(<Skeleton />))
      .map(utilityOf)
      .filter((name) => SHAPE_UTILITY.test(name));

    expect(shapes, "a skeleton draws no size of its own").toEqual([]);
  });

  it("takes the shape a caller passes, including one from a number", () => {
    const markup = renderToStaticMarkup(<Skeleton style={{ aspectRatio: "0.5", width: 176 }} />);
    expect(markup).toContain("aspect-ratio:0.5");
    expect(markup).toContain("width:176px");
  });

  it("sweeps with a transform on a pseudo-element, and stops under reduced motion", () => {
    const classes = classesOf(renderToStaticMarkup(<Skeleton />));
    // §5 rule 2: `transform` and `opacity` only. The sweep is a translated gradient rather than a
    // moving background, which would repaint the surface on every frame. The pseudo-element needs a
    // content value to be generated at all, which is why its absence is a defect and not a detail.
    expect(classes).toContain("after:animate-[shimmer_1.6s_ease-in-out_infinite]");
    expect(classes).toContain("after:content-['']");
    expect(classes).toContain("motion-reduce:after:animate-none");
    expect(classes.some((name) => name.startsWith("after:left-"))).toBe(false);
  });

  it("is decoration, not an announcement", () => {
    // What is loading is said in words by whatever put the screen into this state: a placeholder read
    // aloud is a list of nothing.
    const markup = renderToStaticMarkup(<Skeleton />);
    expect(markup).toContain('aria-hidden="true"');
    expect(markup).not.toContain("role=");
    expect(markup).not.toContain("<svg");
  });
});
