import * as ui from "@peregrine/ui";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { GALLERY_COMPONENTS, Gallery } from "./Gallery.js";

/** The components the gallery's markup actually declares, in the order they appear. */
function shown(): string[] {
  const markup = renderToStaticMarkup(<Gallery />);
  return [...markup.matchAll(/data-component="([^"]+)"/gu)].map((match) => match[1] ?? "");
}

describe("Gallery", () => {
  it("shows every primitive the kit exports", () => {
    // The direction that catches a new primitive: a component added to `packages/ui` and not
    // put on the page would otherwise be discovered by a reviewer who never sees it.
    const exported = Object.entries(ui)
      .filter(([, value]) => typeof value === "function")
      .map(([name]) => name)
      .sort();

    expect([...GALLERY_COMPONENTS].sort()).toEqual(exported);
  });

  it("names only primitives that exist", () => {
    // And the other direction: a rename or a removal leaves a heading over nothing.
    for (const component of GALLERY_COMPONENTS) {
      expect(Object.hasOwn(ui, component), component).toBe(true);
    }
  });

  it("draws a section for each one it declares", () => {
    // The list is data a test reads, so it has to be the list the page renders.
    expect(shown().sort()).toEqual([...GALLERY_COMPONENTS].sort());
  });

  it("shows each primitive once, so a section cannot stand in for two", () => {
    const drawn = shown();
    expect(new Set(drawn).size).toBe(drawn.length);
  });

  it("gives every primitive a description, which is what a gallery is for", () => {
    const markup = renderToStaticMarkup(<Gallery />);

    // One caption per section, and every caption is longer than a word: a heading with no
    // explanation is a screenshot, not documentation.
    expect(markup.match(/text-caption text-text-tertiary/gu)?.length ?? 0).toBeGreaterThanOrEqual(
      GALLERY_COMPONENTS.length,
    );
  });
});
