import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { CARD_SIZES, formatRating, posterVisible, TitleCard } from "./TitleCard.js";

/** Every `class` attribute in the markup, in render order. */
function classAttributes(markup: string): string[][] {
  return [...markup.matchAll(/class="([^"]*)"/gu)].map((match) =>
    (match[1] ?? "").split(/\s+/u).filter((name) => name !== ""),
  );
}

/** The anchor's classes, which are the card frame's: it is the first thing rendered. */
function frameClasses(markup: string): string[] {
  return classAttributes(markup)[0] ?? [];
}

function everyClass(markup: string): string[] {
  return classAttributes(markup).flat();
}

function card(overrides: Partial<Parameters<typeof TitleCard>[0]> = {}): string {
  return renderToStaticMarkup(
    <TitleCard href="/title/movie/42" poster="/cache/abc.webp" title="Alien" {...overrides} />,
  );
}

describe("TitleCard geometry", () => {
  it("reserves the 2:3 box at every size it offers", () => {
    // The acceptance is that no size shifts when an image arrives, so the ratio is asserted at each
    // one rather than at the default alone.
    for (const size of CARD_SIZES) {
      expect(everyClass(card({ size })), size).toContain("aspect-[2/3]");
    }
  });

  it("gives each of the five sizes its own width on §4's 8 px rhythm", () => {
    const widths = CARD_SIZES.map((size) => {
      const width = frameClasses(card({ size })).find((name) => /^w-\[\d+px\]$/u.test(name));
      expect(width, size).toBeDefined();
      return Number(/^w-\[(\d+)px\]$/u.exec(width ?? "")?.[1]);
    });

    expect(new Set(widths).size).toBe(CARD_SIZES.length);
    for (const width of widths) {
      expect(width % 8).toBe(0);
    }
    // Ascending, or a size name would mean nothing: the ladder is the point of having five.
    expect([...widths].sort((left, right) => left - right)).toEqual(widths);
  });

  it("defaults to the middle of the ladder", () => {
    const defaultWidth = frameClasses(card()).find((name) => /^w-\[\d+px\]$/u.test(name));
    const middle = frameClasses(card({ size: "md" })).find((name) => /^w-\[\d+px\]$/u.test(name));
    expect(defaultWidth).toBe(middle);
  });
});

describe("TitleCard poster", () => {
  it("loads the first rail eagerly at high priority and everything else lazily", () => {
    // `react-dom/server` writes the attribute in the casing React's own prop uses — `fetchPriority` —
    // and an HTML parser lowercases it, so the assertion is case-insensitive on purpose: what matters
    // is the priority value, not how React spelled it into the markup.
    const eager = card({ priority: true });
    expect(eager).toContain('loading="eager"');
    expect(eager.toLowerCase()).toContain('fetchpriority="high"');

    const lazy = card();
    expect(lazy).toContain('loading="lazy"');
    expect(lazy.toLowerCase()).toContain('fetchpriority="low"');
  });

  it("starts transparent so the placeholder shows until the bytes decode", () => {
    expect(everyClass(card())).toContain("opacity-0");
    expect(everyClass(card())).not.toContain("opacity-100");
  });

  it("uses the blurhash colour as the placeholder when the caller has one", () => {
    expect(card({ placeholder: "oklch(0.4 0.03 250)" })).toContain(
      'style="background-color:oklch(0.4 0.03 250)"',
    );
  });

  it("draws no image at all when there is no poster to draw", () => {
    expect(card({ poster: null })).not.toContain("<img");
  });

  it("keeps the placeholder when a poster's bytes never arrive", () => {
    // The failing half of the rule, which a static render cannot reach: an evicted blob, a refused
    // path, and a server error are one `onError`, and none of them may leave a hole in the tile.
    expect(posterVisible("/cache/abc.webp", false)).toBe(true);
    expect(posterVisible("/cache/abc.webp", true)).toBe(false);
    expect(posterVisible(null, false)).toBe(false);
    expect(posterVisible(undefined, false)).toBe(false);
    expect(posterVisible("", false)).toBe(false);
  });
});

describe("TitleCard badges", () => {
  it("draws no rating badge when the catalogue has no score", () => {
    expect(card()).not.toContain("<svg");
    expect(card({ rating: null })).not.toContain("<svg");
  });

  it("writes a rating to one decimal beside its mark", () => {
    const markup = card({ rating: 8.25 });
    // The mark is the set's outlined star, not a character: §9 allows no emoji and no glyph standing
    // in for an icon, so the badge draws the same `svg` every other mark in the app is.
    expect(markup).toContain('<svg aria-hidden="true"');
    expect(markup).toContain("8.3");
    expect(formatRating(8)).toBe("8.0");
  });

  it("draws the availability chip only for a state that has a signal and a word", () => {
    const acquiring = card({ availability: "acquiring", availabilityLabel: "Downloading" });
    expect(acquiring).toContain("Downloading");
    expect(everyClass(acquiring)).toContain("bg-state-info");

    const failing = card({ availability: "unavailable", availabilityLabel: "Unavailable" });
    expect(everyClass(failing)).toContain("bg-state-danger");

    // A quiet state wears nothing, which is four of the nine by design.
    for (const quiet of ["discovered", "watchlisted", "available"] as const) {
      expect(everyClass(card({ availability: quiet, availabilityLabel: "Should not show" }))).not.toContain(
        "bg-state-info",
      );
      expect(card({ availability: quiet, availabilityLabel: "Should not show" })).not.toContain(
        "Should not show",
      );
    }
  });

  it("holds no copy of its own for a state", () => {
    // The kit is a leaf with no i18n, so a chip without the caller's word is not drawn rather than
    // guessed at in English.
    expect(card({ availability: "acquiring" })).not.toContain("Downloading");
  });

  it("draws no chip for an empty word, which would be colour alone", () => {
    // Found by looking at the gallery: an empty label rendered a chip carrying nothing but its dot,
    // and §2.2 requires a state indicator to be paired with text or an icon.
    expect(everyClass(card({ availability: "acquiring", availabilityLabel: "" }))).not.toContain(
      "bg-state-info",
    );
  });

  it("keeps the chip inside the tile however long the word is", () => {
    // The smallest tile is 88 px: a chip that sized itself to its text would run off the poster and
    // be clipped mid-word by the frame's own overflow.
    const classes = everyClass(card({ availability: "acquiring", availabilityLabel: "Wird heruntergeladen" }));
    expect(classes).toContain("max-w-[calc(100%-0.5rem)]");
    expect(classes).toContain("truncate");
  });
});

describe("TitleCard accessible name", () => {
  it("names itself from the parts it was given, and announces each once", () => {
    const markup = card({
      availability: "acquiring",
      availabilityLabel: "Downloading",
      meta: "1979 · Film",
      rating: 8.5,
      ratingLabel: "rated 8.5 out of 10",
    });

    expect(markup).toContain(
      '<span class="sr-only">Alien, 1979 · Film, rated 8.5 out of 10, Downloading</span>',
    );
    // The mark and the chip are decorative: the name above is what a screen reader reads, and an
    // `alt` or a second label would say it all twice.
    expect(markup).toContain('alt=""');
    expect(markup).toContain('aria-hidden="true"');
  });

  it("draws the title nowhere the eye can see it", () => {
    // PG-E7-05 owns the hover metadata panel; a caption here would be a second copy of the name and
    // would change the tile's geometry, which is the one thing this component exists to hold still.
    expect(card({ meta: "1979 · Film" }).match(/Alien/gu)).toHaveLength(1);
  });

  it("leaves out the parts the caller did not supply", () => {
    expect(card({ title: "Alien" })).toContain('<span class="sr-only">Alien</span>');
  });

  it("carries the destination in the markup, so a context menu can offer it", () => {
    expect(card()).toContain('href="/title/movie/42"');
  });

  it("lets the call site win a conflicting utility", () => {
    expect(frameClasses(card({ className: "w-[999px]" }))).toContain("w-[999px]");
    expect(frameClasses(card({ className: "w-[999px]" }))).not.toContain("w-[144px]");
  });
});
