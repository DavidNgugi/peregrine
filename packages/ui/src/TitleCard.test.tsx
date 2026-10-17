import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import {
  CARD_ASPECT,
  CARD_SIZES,
  CARD_WIDTHS,
  formatRating,
  posterVisible,
  previewOverview,
  TitleCard,
  TitlePreviewPanel,
} from "./TitleCard.js";

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

  it("keeps `CARD_WIDTHS` and the classes it draws in step", () => {
    // The map exists for the rail, which has to leave room for a tile it cannot measure — its items
    // are the caller's. Two statements of the same numbers are two things to drift, and the rail
    // would silently under-reserve rather than fail, so this is where they are held together.
    for (const size of CARD_SIZES) {
      const drawn = frameClasses(card({ size })).find((name) => /^w-\[\d+px\]$/u.test(name));
      expect(drawn, size).toBe(`w-[${String(CARD_WIDTHS[size])}px]`);
    }

    // And the height the rail derives from them is the ratio the frame draws.
    expect(CARD_ASPECT).toBe(2 / 3);
    expect(everyClass(card())).toContain("aspect-[2/3]");
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
      expect(
        everyClass(card({ availability: quiet, availabilityLabel: "Should not show" })),
      ).not.toContain("bg-state-info");
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
    const classes = everyClass(
      card({ availability: "acquiring", availabilityLabel: "Wird heruntergeladen" }),
    );
    expect(classes).toContain("max-w-[calc(100%-0.5rem)]");
    expect(classes).toContain("truncate");
  });
});

describe("TitleCard download mark", () => {
  it("draws a top border whose tone is the status's", () => {
    expect(everyClass(card({ downloadStatus: "active", downloadLabel: "Downloading" }))).toContain(
      "bg-accent",
    );
    expect(everyClass(card({ downloadStatus: "done", downloadLabel: "Downloaded" }))).toContain(
      "bg-state-ok",
    );
    expect(everyClass(card({ downloadStatus: "failed", downloadLabel: "Failed" }))).toContain(
      "bg-state-danger",
    );
  });

  it("holds no copy of its own for a download", () => {
    // A status with no caller's word draws nothing, rather than guessing at English.
    const markup = card({ downloadStatus: "active" });
    expect(markup).not.toContain("Downloading");
    expect(everyClass(markup)).not.toContain("bg-accent");
  });

  it("draws no border for an empty word, which would be colour alone", () => {
    expect(everyClass(card({ downloadStatus: "done", downloadLabel: "" }))).not.toContain(
      "bg-state-ok",
    );
  });

  it("adds the download's word to the accessible name exactly once", () => {
    const markup = card({ downloadStatus: "active", downloadLabel: "Downloading" });
    expect(markup).toContain('<span class="sr-only">Alien, Downloading</span>');
    expect(markup.match(/Downloading/gu)).toHaveLength(1);
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

  it("draws the title nowhere the eye can see it unless the caller asks for it", () => {
    // PG-E7-05 owns the hover metadata panel; a caption here would be a second copy of the name and
    // would change the tile's geometry, which is the one thing this component exists to hold still.
    // **And `showTitle` is the one caller that needs the name visible**: a rail's poster identifies its
    // title and a captured frame does not, so a library tiled with frames is a wall a reader cannot read.
    expect(card({ meta: "1979 · Film" }).match(/Alien/gu)).toHaveLength(1);
    // Asked for or not, the *resting* tile is identical: the name lives in the panel, and the panel exists
    // only once the interaction has fired — which this renderer never does, and which is why the panel is
    // asserted directly below rather than through a hover.
    expect(card({ showTitle: true })).toBe(card({ showTitle: false }));
  });

  it("names the title in the panel, in one clamped line, when the caller asks for it", () => {
    // **The half a static render can reach.** `TitlePreviewPanel` is rendered on its own because the tile
    // never draws it without a pointer — and a library tile needs the name: a captured frame identifies
    // nothing, and the meta line beside it is long enough to wrap and push the action out of the panel on
    // its own. Both lines are `truncate` for that reason, and this is the assertion that they are.
    const named = renderToStaticMarkup(
      <TitlePreviewPanel
        detailsLabel="View More"
        meta="Film · 3 GB · No metadata yet"
        name="Alien"
      />,
    );
    const unnamed = renderToStaticMarkup(
      <TitlePreviewPanel detailsLabel="View More" meta="Film · 3 GB · No metadata yet" />,
    );

    expect(named).toContain("Alien");
    expect(unnamed).not.toContain("Alien");
    expect(named.match(/truncate/gu)?.length).toBe(2);
    // The action still fits: the panel is a fixed-width box and both text lines are clamped, so nothing
    // is pushed past its edge — the failure a screenshot of a fresh library showed.
    expect(named).toContain("View More");
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

/**
 * The hover preview's markup and its animation contract.
 *
 * **The one thing this file cannot do is interact.** The test environment is Node with
 * `react-dom/server`: there is no pointer, no timer, no `IntersectionObserver`, and no
 * `transitionend`, so the marks a hover would leave are decided in `hover-preview.ts` and tested
 * there. What is asserted here is what a *static* render can hold to the rules: the layer that is
 * not there before anything happened, the classes that keep the animation on the compositor, and the
 * panel's own markup for each combination of data the caller supplies.
 */
describe("TitleCard hover preview", () => {
  it("draws no metadata layer before the interaction has fired", () => {
    // The ladder's first state, reached without a pointer: a resting tile carries no layer at all
    // rather than a hidden one, so nothing is painted and nothing is tabbable.
    const markup = card({
      detailsLabel: "Details",
      meta: "1979 · Film",
      overview: "A crew answers a distress call.",
    });

    expect(markup).not.toContain("data-preview-panel");
    expect(markup).not.toContain("Details");
  });

  it("rests at scale 1 with nothing promoted to the compositor", () => {
    // §5 rule 3: `will-change` is applied when an interaction starts and removed when it settles —
    // never left on a grid of five thousand tiles.
    const markup = card();

    expect(markup).toContain("transform:scale(1)");
    expect(markup).not.toContain("will-change");
  });

  it("rests in document order, with no lift of its own", () => {
    // The lift is the revealed tile's (`previewLift`), and it is inline because it is a value that
    // must be *absent* rather than empty on every tile that is not enlarged. What a static render can
    // say is that no tile claims a stacking order before the interaction: the revealed half is the
    // state machine's, and the browser is what showed it working.
    expect(card()).not.toContain("z-index");
  });

  it("animates transform and opacity and nothing else", () => {
    const classes = frameClasses(card());

    expect(classes).toContain("transition-transform");
    expect(classes).toContain("after:transition-opacity");
    for (const forbidden of [
      "transition-all",
      "transition-shadow",
      "transition-[width]",
      "transition-[height]",
      "transition-[box-shadow]",
    ]) {
      expect(classes).not.toContain(forbidden);
    }
  });

  it("rides the shadow ramp on a pseudo-element rather than animating a box-shadow", () => {
    // §5's rule, and the reason it exists: an animated `box-shadow` repaints the whole tile every
    // frame. The ramp is the `::after`'s opacity, so the shadow itself never animates.
    const classes = frameClasses(card());

    expect(classes.some((name) => name.startsWith("after:shadow-"))).toBe(true);
    expect(classes.some((name) => name.startsWith("after:opacity-"))).toBe(true);
    expect(classes.filter((name) => /(^|:)shadow-/u.test(name))).toHaveLength(1);
  });

  it("does not clip the ramp's own shadow with the containment hint", () => {
    // Found while wiring this: `contain: paint` clips a descendant's painting to the tile's box, and
    // the shadow ramp *is* a descendant — its whole point is the part that falls outside the border
    // box. The poster is clipped by the inner frame instead, which is where the rounded corner is.
    const classes = frameClasses(card());

    expect(classes).toContain("[contain:layout_style]");
    expect(classes).not.toContain("[contain:layout_paint_style]");
    expect(everyClass(card())).toContain("overflow-hidden");
  });

  it("names the motion tokens, with §5's numbers as the fallback until they land", () => {
    // The `--dur-*`/`--ease-*` families are PG-E21-07's and do not exist yet, so the ladder has to be
    // right today and pick the tokens up unchanged the day they do. The ramp's pair is in the
    // classes, which is how a resting tile still states it.
    const markup = card();
    const classes = frameClasses(card());

    expect(markup).toContain("var(--dur-2, 120ms)");
    expect(markup).toContain("var(--ease-accelerate");
    expect(classes.some((name) => name.includes("var(--dur-3,180ms)"))).toBe(true);
    expect(classes.some((name) => name.includes("var(--ease-standard"))).toBe(true);
  });

  it("has a reduced-motion branch for both transitions", () => {
    // §5 rule 5: the durations collapse, and the reveal is instant rather than absent — the panel
    // still appears, it just stops moving. This is the kit's established branch, in `Button`,
    // `Progress`, and `Sidebar` too.
    const classes = frameClasses(card());

    expect(classes).toContain("motion-reduce:transition-none");
    expect(classes).toContain("motion-reduce:after:transition-none");
  });
});

describe("TitlePreviewPanel", () => {
  function panel(overrides: Partial<Parameters<typeof TitlePreviewPanel>[0]> = {}): string {
    return renderToStaticMarkup(<TitlePreviewPanel {...overrides} />);
  }

  it("draws the score with its mark, the meta line, the prose, and the Details verb", () => {
    const markup = panel({
      detailsLabel: "Details",
      meta: "1999 · Film",
      overview: "A hacker learns the truth.",
      rating: 8.2,
    });

    expect(markup).toContain('data-preview-panel=""');
    expect(markup).toContain('<svg aria-hidden="true"');
    expect(markup).toContain("8.2");
    expect(markup).toContain("1999 · Film");
    expect(markup).toContain("A hacker learns the truth.");
    expect(markup).toContain("Details");
  });

  it("keeps the upper poster visible and reserves a separate metadata surface", () => {
    const markup = panel({ overview: "Prose", detailsLabel: "Details" });
    expect(markup).toContain("bottom-0");
    expect(markup).toContain("max-h-full");
    expect(markup).toContain("from-transparent");
    expect(markup).toContain("w-full items-center justify-center");
  });

  it("hides itself from assistive technology rather than naming the tile twice", () => {
    // Everything here is already in the tile's accessible name; the synopsis is what a person chose
    // to hover for, and announcing it inside the link would make the link's name a paragraph.
    expect(panel({ overview: "Prose", rating: 8.2 })).toContain('aria-hidden="true"');
  });

  it("adds no control of its own, so the tile stays one tab stop", () => {
    // PG-E9-01 owns the watchlist and bookmark actions, and an enabled button that does nothing is
    // worse than an absent one (PG-E1-07). Details is the tile's own navigation, drawn as part of
    // the link rather than as a nested control inside it.
    const markup = panel({ detailsLabel: "Details", rating: 8.2 });

    expect(markup).not.toContain("<button");
    expect(markup).not.toContain("<a ");
  });

  it("leaves out every line it has no data for", () => {
    const empty = panel({});

    expect(empty).not.toContain("<svg");
    expect(empty).not.toContain("Details");
    // The layer is still drawn — an empty panel is the *reveal* of a title with nothing to say, and
    // the surface that covers the artwork is what the reveal is.
    expect(empty).toContain('data-preview-panel=""');
  });

  it("draws a score on its own, and never a count", () => {
    // `votes` is not a prop any more: a tile is judged by its artwork and its score, and the count was
    // the longest string the panel drew. Asserted as an absence so a reintroduction is a failure.
    const scoreOnly = panel({ rating: 7 });
    expect(scoreOnly).toContain("7.0");
    expect(scoreOnly).not.toContain("votes");
    expect(panel({})).not.toContain("votes");
  });

  it("clamps the prose to three lines and never cuts a word out of it", () => {
    // The clamp is CSS, which drops whole lines at a word boundary. The text itself is handed over
    // intact — a kit that cut "The Matrix is a…sci" would be building ellipsis soup.
    const prose = "A hacker learns the truth about the world he lives in.";
    const markup = panel({ overview: prose });

    expect(everyClass(markup)).toContain("line-clamp-3");
    expect(markup).toContain(prose);
    expect(markup).not.toContain("…");
  });
});

describe("previewOverview", () => {
  it("says nothing at all for nothing at all", () => {
    expect(previewOverview(null)).toBeNull();
    expect(previewOverview(undefined)).toBeNull();
    expect(previewOverview("")).toBeNull();
    expect(previewOverview("   ")).toBeNull();
    expect(previewOverview("\n\t ")).toBeNull();
  });

  it("collapses the provider's own whitespace so the clamp counts lines and not newlines", () => {
    expect(previewOverview("  A hacker\n\nlearns   the truth.  ")).toBe(
      "A hacker learns the truth.",
    );
  });

  it("hands real prose over untouched", () => {
    expect(previewOverview("A crew answers a distress call.")).toBe(
      "A crew answers a distress call.",
    );
  });
});
