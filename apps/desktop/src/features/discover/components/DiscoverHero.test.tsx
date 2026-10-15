import { createI18n } from "@peregrine/i18n";
import { renderToStaticMarkup } from "react-dom/server";
import { I18nextProvider } from "react-i18next";
import { describe, expect, it } from "vitest";

import { DiscoverHero, type HeroSlide } from "./DiscoverHero.js";

const i18n = createI18n();

/**
 * The hero as a static render draws it.
 *
 * **The rotation itself is not here, and that is the honest split**: `renderToStaticMarkup` is one
 * pass with no timers, so a test cannot watch a slide change. What it *can* hold is the shape the
 * cross-fade needs — every slide's image in the DOM at once, one at full opacity and the rest at
 * zero, a transition over §5's token rather than a literal, and the branch that makes the change
 * instant under `prefers-reduced-motion` — plus the copy, the actions, and the empty and single-slide
 * rows. The decisions behind them are `hero-rotation.test.ts`'s.
 */
function slide(overrides: Partial<HeroSlide> = {}): HeroSlide {
  return {
    id: 1,
    kind: "movie",
    name: "The Matrix",
    meta: "1999 · Film",
    href: "/title/movie/1",
    // `null` rather than a URL: a Node test has no asset protocol, and the hero draws its reserved
    // surface either way.
    backdrop: null,
    poster: null,
    overview: null,
    score: null,
    scoreLabel: null,
    playable: false,
    ...overrides,
  };
}

function hero(
  slides: readonly HeroSlide[],
  props: Partial<Parameters<typeof DiscoverHero>[0]> = {},
): string {
  return renderToStaticMarkup(
    <I18nextProvider i18n={i18n}>
      <DiscoverHero onOpen={() => undefined} slides={slides} {...props} />
    </I18nextProvider>,
  );
}

/** The Play button's own markup, which is where the disabled state and its reason live. */
function playButton(markup: string): string {
  const start = markup.indexOf("<button");
  const end = markup.indexOf("</button>", start);
  return markup.slice(start, end);
}

describe("DiscoverHero", () => {
  it("keeps the toolbar over the artwork and reserves room above the copy", () => {
    const markup = hero([slide()], { toolbar: <form aria-label="Search overlay" /> });
    expect(markup).toContain("absolute inset-x-0 top-0 z-20 p-4");
    expect(markup).toContain("pt-28 sm:pt-20");
    expect(markup).toContain("Search overlay");
  });

  it("reserves the hero height and separates the trailing strip from the copy", () => {
    const markup = hero([slide(), slide({ id: 2 })]);
    expect(markup).toContain("shrink-0");
    expect(markup).toContain("min-h-[28rem]");
    expect(markup).toContain("bg-linear-to-r");
    expect(markup).toContain("lg:justify-between");
    expect(markup).toContain("grid-cols-5 gap-2 lg:w-[42%]");
  });

  it("draws the active slide's copy and the actions that go with it", () => {
    const markup = hero([
      slide({
        overview: "A hacker learns the truth.",
        score: "8.2",
        scoreLabel: "rated 8.2 out of 10",
      }),
      slide({ id: 2, name: "Inception", href: "/title/movie/2" }),
    ]);

    expect(markup).toContain("Featured title");
    expect(markup).toContain("The Matrix");
    expect(markup).toContain("1999 · Film");
    expect(markup).toContain("8.2");
    expect(markup).toContain("rated 8.2 out of 10");
    expect(markup).toContain("A hacker learns the truth.");
    // "Play" is the control's name, not its label: the hero draws the mark alone and the word lives in
    // `aria-label` and the tooltip, which is what this assertion is reading.
    expect(markup).toContain("Play");
    expect(markup).toContain("View More");
    expect(markup).toContain('href="/title/movie/1"');
  });

  it("carries no chip the core cannot honestly fill", () => {
    // TMDB's list answers send no runtime and no genre, so the hero draws neither: the meta line is
    // exactly the caller's, and the only number on it is the score.
    const markup = hero([slide({ meta: "Series", score: null })]);

    expect(markup).toContain("Series");
    expect(markup.replace(/<[^>]*>/gu, "")).not.toMatch(/\d+\s*min/gu);
    expect(markup).not.toContain("Genre");
  });

  it("stacks every slide and fades them over §5's whole-surface duration", () => {
    // The cross-fade: the outgoing image is still in the DOM while the incoming one arrives, only
    // `opacity` moves, and the duration is the token with §5's number as the fallback until
    // PG-E21-07 lands it. `motion-reduce:transition-none` is the branch that makes the change
    // instant under `prefers-reduced-motion` rather than absent (§5 rule 5).
    const markup = hero([
      slide({ backdrop: "/cache/one", id: 1 }),
      slide({ backdrop: "/cache/two", id: 2 }),
      slide({ backdrop: "/cache/three", id: 3 }),
    ]);
    // The hero's own layers, not the strip's thumbnails: those are a second row of images with
    // their own classes, and counting them would hide a missing layer.
    const images = [...markup.matchAll(/<img[^>]*>/gu)]
      .map((match) => match[0])
      .filter((image) => image.includes("absolute inset-0 size-full"));

    expect(images).toHaveLength(3);
    expect(images[0]).toContain("opacity-100");
    expect(images[1]).toContain("opacity-0");
    expect(images[2]).toContain("opacity-0");
    for (const image of images) {
      expect(image).toContain("object-cover");
      expect(image).toContain("transition-opacity");
      expect(image).toContain("duration-[var(--dur-5,320ms)]");
      expect(image).toContain("motion-reduce:transition-none");
      // §5 rule 2: opacity and transform only. Nothing here transitions a layout property.
      expect(image).not.toContain("transition-all");
    }
    expect(markup).toContain("aspect-video");
    expect(markup).toContain("overflow-hidden");
  });

  it("falls back to the poster, cropped, and to the reserved surface when there is neither", () => {
    // The wide image is what the hero is *for*, and a title whose answer carried none is a state:
    // `object-cover` crops rather than stretches, and a title with no artwork at all draws the
    // surface under the scrim rather than a hole or a broken image.
    const poster = hero([slide({ poster: "/cache/poster" })]);
    expect(poster).toContain('src="/cache/poster"');
    expect(poster).toContain("object-cover");

    const bare = hero([slide()]);
    expect(bare).not.toContain("<img");
    expect(bare).toContain("bg-surface-2");
    expect(bare).toContain("bg-linear-to-t");
  });

  it("disables Play with the title's own reason when there is no file", () => {
    const markup = hero([slide({ playable: false })]);

    expect(playButton(markup)).toContain('disabled=""');
    expect(markup).toContain("Not in your library yet");
    expect(markup).not.toContain("Playback is not available yet");
  });

  it("enables Play only when there is a file and something to play it with", () => {
    // The two facts are separate, and the screen must not confuse them: a file with no playback
    // surface in this build is disabled with the build's reason, and a build that can play hands
    // over a handler and gets a live control.
    const noPlayer = hero([slide({ playable: true })]);
    expect(playButton(noPlayer)).toContain('disabled=""');
    expect(noPlayer).toContain("Playback is not available yet");

    const playable = hero([slide({ playable: true })], { onPlay: () => undefined });
    expect(playButton(playable)).not.toContain('disabled=""');
    expect(playable).not.toContain("Playback is not available yet");
  });

  it("draws the strip only when there is somewhere to jump to", () => {
    // A strip of one is a control that re-shows the slide already up, which `PG-E1-07` says must be
    // absent rather than inert.
    const many = hero([
      slide({ id: 1, name: "One" }),
      slide({ id: 2, name: "Two" }),
      slide({ id: 3, name: "Three" }),
    ]);
    expect([...many.matchAll(/aria-current="true"/gu)]).toHaveLength(1);
    expect(many).toContain('aria-label="Show Two"');
    expect(many).toContain('aria-label="Show Three"');

    const one = hero([slide()]);
    expect(one).not.toContain("aria-current");
    expect(one).not.toContain("Featured titles");
  });

  it("draws nothing at all for an empty rotation", () => {
    // The screen decides whether to mount the hero; this is the component's own answer for a list
    // that arrived empty anyway, and it is nothing rather than an empty band.
    expect(hero([])).toBe("");
  });
});
