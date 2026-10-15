import { Icon, IconButton, Link, Skeleton } from "@peregrine/ui";
import { useEffect, useState, type ReactElement, type ReactNode } from "react";
import { useTranslation } from "react-i18next";

import type { RailItemDto } from "@peregrine/ipc-client";

import { heroPlay } from "../../../lib/play-action.js";

import {
  advance,
  currentSlide,
  FIRST_SLIDE,
  HERO_INTERVAL_MS,
  rotationRuns,
  selectSlide,
  type RotationState,
} from "./hero-rotation.js";

/**
 * The hero: a full-bleed backdrop with the title's own copy over it, cycling through the first
 * rail's items.
 *
 * **The image is the backdrop and not the poster.** A poster is a portrait 2:3 image, and stretched
 * across a hero it is either a crop of somebody's chin or a pair of letterbox bars; the backdrop is
 * the wide image the provider drew for exactly this band, and it has been in the cache since
 * `PG-E6-09` (`w780`, against the poster's `w342`). A title whose answer carried no wide image falls
 * back to its poster — still `object-cover`, so it is cropped rather than stretched — and a title
 * with neither draws the reserved surface under the scrim, which is a state rather than a hole.
 *
 * **The box is reserved before any bytes arrive** (§4's rule, and the reason there is no layout
 * shift): `aspect-video` fixes the height, every image is absolutely positioned inside it, and the
 * copy is in flow so a long synopsis or 1.4× text expansion makes the hero *grow* rather than
 * overflow it.
 *
 * **The cross-fade is the whole rotation.** Every slide's image is in the DOM at once, stacked and
 * absolutely positioned; moving between them is `opacity` over §5's whole-surface duration
 * (`--dur-5`), so the outgoing image fades out while the incoming one fades in — one layer, no
 * unmount, no flash (`DESIGN_SYSTEM §11` rules 6 and 2), and nothing but `opacity` is animated. The
 * duration is a token with §5's number as its fallback (the `--dur-*` family is `PG-E21-07`'s and has
 * not landed), and `motion-reduce:transition-none` is the same branch `Button`, `Progress`, and the
 * hover ladder carry: under `prefers-reduced-motion` the *change is instant while the content
 * stays*, which is §5 rule 5's "essential state changes become instant rather than absent".
 *
 * **The timing is `hero-rotation.ts`'s and not this file's.** Which slide is next, what the end of
 * the list does, what a click on the strip does to the schedule, and what happens with one item or
 * none are pure functions with their own tests, because this runner is Node with `react-dom/server`
 * and a timer is not something a test here can drive.
 */
export interface HeroSlide {
  /** The catalogue row, which is the stable key the strip is drawn from. */
  readonly id: RailItemDto["titleId"];
  readonly name: string;
  /** Film or series, which is what the route to the title's own screen needs. */
  readonly kind: RailItemDto["kind"];
  /** The caller's one line: "1999 · Film". */
  readonly meta: string;
  /** Where the Details action goes. */
  readonly href: string;
  /** The cached backdrop, as a URL, when there is one. */
  readonly backdrop: string | null;
  /** The cached poster, as a URL, which stands in when there is no backdrop. */
  readonly poster: string | null;
  readonly overview: string | null;
  /** The score as it is drawn, such as "8.2", or `null` when the provider gave none. */
  readonly score: string | null;
  /** The score as it is spoken, such as "rated 8.2 out of 10". */
  readonly scoreLabel: string | null;
  /** The core's answer about whether the library holds a file for this title. */
  readonly playable: boolean;
}

export interface DiscoverHeroProps {
  /** The rotation, already chosen and resolved (`heroSlides` and the caller's own formatting). */
  readonly slides: readonly HeroSlide[];
  readonly toolbar?: ReactNode;
  /**
   * What Play does, when this build can play anything at all.
   *
   * Omitted by the only caller there is, deliberately: nothing in this build has a playback surface
   * (`PG-E15`), and a control that fired nothing would be the inert button `PG-E1-07` forbids — so
   * its absence is what disables Play with a stated reason rather than leaving it live and empty.
   */
  readonly onPlay?: (slide: HeroSlide) => void;
  readonly onOpen: (slide: HeroSlide) => void;
  /**
   * Whether the band runs to the window's own edges.
   *
   * **Discover passes `true`, and the numbers are the screen's inset written the other way.** A hero
   * inset by 24 px on the sides and 16 above reads as a card floating on the page rather than as the
   * page's own artwork, and the reference has it full-bleed with the toolbar over it. The margins
   * cancel `Screen`'s `px-6 py-4` exactly, and the rounding goes with them: a corner radius belongs to
   * a box, and this is no longer one.
   */
  readonly bleed?: boolean;
}

/** The hero, from slides the caller has already resolved. */
export function DiscoverHero({
  slides,
  onPlay,
  onOpen,
  toolbar,
  bleed = false,
}: DiscoverHeroProps): ReactElement | null {
  const { t } = useTranslation();
  const [rotation, setRotation] = useState<RotationState>(FIRST_SLIDE);
  const count = slides.length;
  const active = currentSlide(rotation, count);
  const slide = slides[active];

  // The schedule, re-armed on every move: `rotation.epoch` changes whether the interval fired or a
  // person clicked, so a click is answered with a whole interval from the click. Keying the effect
  // on the *index* would do the same thing; keying it on nothing — the trap — would leave a timer
  // armed across renders that a click could then beat by a few milliseconds.
  useEffect(() => {
    if (!rotationRuns(count)) {
      return;
    }
    const timer = window.setInterval(() => {
      setRotation((current) => advance(current, count));
    }, HERO_INTERVAL_MS);
    return () => {
      window.clearInterval(timer);
    };
  }, [rotation.epoch, count]);

  if (slide === undefined) {
    return null;
  }

  const play = heroPlay(slide.playable, onPlay !== undefined);

  return (
    <section
      aria-label={t("discover.hero.label")}
      className={[
        "relative shrink-0 overflow-hidden bg-surface-2",
        bleed ? "-mx-6 -mt-4" : "rounded-md",
      ].join(" ")}
    >
      {toolbar === undefined ? null : (
        <div className="absolute inset-x-0 top-0 z-20 p-4">{toolbar}</div>
      )}
      <div
        className={
          "relative flex aspect-video min-h-[28rem] w-full flex-col justify-end" +
          (toolbar === undefined ? "" : " pt-28 sm:pt-20")
        }
      >
        {slides.map((item) => {
          const source = item.backdrop ?? item.poster;
          return source === null ? null : (
            <img
              alt=""
              className={[
                "absolute inset-0 size-full object-cover",
                // §5 rule 1: a whole surface moving, so `--dur-4`/`--dur-5` and not a control's
                // duration. The token is named with §5's own number as the fallback until it lands.
                "transition-opacity duration-[var(--dur-5,320ms)] ease-[var(--ease-emphasized,cubic-bezier(0.05,0.7,0.1,1))]",
                "motion-reduce:transition-none",
                item.id === slide.id ? "opacity-100" : "opacity-0",
              ].join(" ")}
              decoding="async"
              fetchPriority={item.id === slides[0]?.id ? "high" : "low"}
              key={item.id}
              loading={item.id === slides[0]?.id ? "eager" : "lazy"}
              src={source}
            />
          );
        })}
        {/* The scrim, so the copy is legible over *any* artwork. A gradient of the page's own
            surface rather than `--surface-scrim`: that token is a dimming layer — dark in both
            themes — while `--text-primary` is light in one and dark in the other, so text over it has
            no validated pairing (the same finding the hover panel records). */}
        <div
          aria-hidden="true"
          className="absolute inset-0 bg-linear-to-t from-surface-0 via-surface-0/70 to-transparent"
        />
        <div
          aria-hidden="true"
          className="absolute inset-0 bg-linear-to-r from-surface-0 via-surface-0/40 to-transparent"
        />
        <div className="relative flex flex-col gap-6 p-6 lg:flex-row lg:items-end lg:justify-between">
          <div className="flex min-w-0 flex-col gap-3 lg:max-w-[55%]">
            <h2 className="text-display text-text-primary">{slide.name}</h2>
            <p className="flex flex-wrap items-baseline gap-x-3 gap-y-1 text-body text-text-secondary">
              <span>{slide.meta}</span>
              {slide.score === null ? null : (
                <span className="inline-flex items-center gap-1 text-text-primary tabular-nums">
                  <Icon className="text-state-warn" name="star" size={16} />
                  <span aria-hidden="true">{slide.score}</span>
                  <span className="sr-only">{slide.scoreLabel ?? slide.score}</span>
                </span>
              )}
            </p>
            {slide.overview === null ? null : (
              <p className="line-clamp-3 max-w-[60ch] text-body text-text-secondary">
                {slide.overview}
              </p>
            )}
            <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
              {/* **A round control, and the mark rather than the word.** Play is the one action on this
                  band, and a filled circle with a triangle is what it is everywhere a person has met one
                  — the word beside it was the last thing on a line that already says the title, the year,
                  the score, and the synopsis. `size-9` is the height the control beside it draws at, so
                  the two are one row rather than two sizes, and the name it needs is still there for a
                  screen reader (and in the tooltip). */}
              <IconButton
                className="size-9 rounded-full"
                disabled={play.kind === "unavailable"}
                icon="play"
                label={t("discover.hero.play")}
                onClick={() => {
                  onPlay?.(slide);
                }}
                title={t("discover.hero.play")}
                variant="primary"
              />
              {/* A link rather than a button, because it says where it goes and a context menu can
                offer it; `variant` is what keeps it looking like the control beside it. The
                information mark is the same signal the reference puts there: the label says *more*,
                and the glyph says more of what. */}
              <Link
                className="rounded-full"
                href={slide.href}
                onNavigate={() => {
                  onOpen(slide);
                }}
                variant="secondary"
              >
                <Icon name="info" size={20} />
                {t("card.details")}
              </Link>
              {play.kind === "play" ? null : (
                <p className="text-caption text-text-secondary" role="status">
                  {t(play.reason === "noFile" ? "discover.hero.noFile" : "discover.hero.noPlayer")}
                </p>
              )}
            </div>
          </div>
          {/* A rotation of one has no strip: a row of one control that re-shows the slide already
              up is a control that does nothing, and `PG-E1-07` says absent rather than inert. */}
          {rotationRuns(count) ? (
            <ul
              aria-label={t("discover.hero.strip")}
              className="grid w-full max-w-md shrink-0 grid-cols-5 gap-2 lg:w-[42%]"
            >
              {slides.map((item, index) => {
                const thumb = item.backdrop ?? item.poster;
                return (
                  <li key={item.id}>
                    <button
                      aria-current={index === active}
                      aria-label={t("discover.hero.show", { name: item.name })}
                      className={[
                        "block aspect-video w-full overflow-hidden rounded-md border-2",
                        "focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-focus-ring",
                        index === active
                          ? "border-accent opacity-100"
                          : "border-accent/40 opacity-70 hover:opacity-100",
                      ].join(" ")}
                      onClick={() => {
                        setRotation((current) => selectSlide(current, index, count));
                      }}
                      type="button"
                    >
                      {thumb === null ? (
                        <span className="block size-full bg-surface-2" />
                      ) : (
                        <img
                          alt=""
                          className="size-full object-cover"
                          decoding="async"
                          loading="lazy"
                          src={thumb}
                        />
                      )}
                    </button>
                  </li>
                );
              })}
            </ul>
          ) : null}
        </div>
      </div>
    </section>
  );
}

/**
 * The hero's own geometry, waiting for the rails that will fill it (docs/DESIGN_SYSTEM.md §11 rule 2:
 * "cached content or a skeleton with real geometry").
 *
 * **Drawn rather than omitted, and that is the decision worth stating.** An empty catalogue draws no
 * hero at all — that is [`DiscoverHero`]'s rule and it stays, because a band of nothing above the
 * rails is a heading for content that does not exist. The *first fetch* of a session is the other
 * case: the content does exist and is on its way, the toolbar is already on screen, and the rails
 * below are drawing their own placeholders — so a missing band here would move everything the moment
 * the hero arrived. The box, the toolbar's padding, and the trailing strip are the hero's own, which
 * makes the swap a cross-fade rather than a re-layout.
 *
 * The placeholders inside are decoration; `aria-busy` on the region is what says "still filling", and
 * the line the screen puts above the rails says what is being fetched, in words.
 */
export function DiscoverHeroSkeleton({
  toolbar,
  bleed = false,
}: {
  readonly toolbar?: ReactNode;
  /** Whether the band runs to the window's own edges, for the same reason [`DiscoverHero`] does. */
  readonly bleed?: boolean;
}): ReactElement {
  const { t } = useTranslation();

  return (
    <section
      aria-busy="true"
      aria-label={t("discover.hero.label")}
      className={[
        "relative shrink-0 overflow-hidden bg-surface-2",
        bleed ? "-mx-6 -mt-4" : "rounded-md",
      ].join(" ")}
    >
      {toolbar === undefined ? null : (
        <div className="absolute inset-x-0 top-0 z-20 p-4">{toolbar}</div>
      )}
      <div
        className={
          "relative flex aspect-video min-h-[28rem] w-full flex-col justify-end" +
          (toolbar === undefined ? "" : " pt-28 sm:pt-20")
        }
      >
        <Skeleton className="absolute inset-0 rounded-none" />
        <div className="relative flex flex-col gap-6 p-6 lg:flex-row lg:items-end lg:justify-between">
          <div aria-hidden="true" className="flex min-w-0 flex-col gap-3 lg:max-w-[55%]">
            {/* The copy's blocks: a display line, the meta line, and the three lines of synopsis the
                real hero clamps to (`line-clamp-3`), so the band's height does not depend on how long
                the first title's synopsis turns out to be. */}
            <Skeleton className="h-9 w-4/5 rounded-sm" />
            <Skeleton className="h-5 w-2/5 rounded-sm" />
            <Skeleton className="h-4 w-full max-w-[60ch] rounded-sm" />
            <Skeleton className="h-4 w-11/12 max-w-[60ch] rounded-sm" />
            <Skeleton className="h-4 w-2/3 max-w-[60ch] rounded-sm" />
            <div className="flex flex-wrap items-center gap-3 pt-1">
              <Skeleton className="h-10 w-28 rounded-full" />
              <Skeleton className="h-10 w-32 rounded-full" />
            </div>
          </div>
          <div
            aria-hidden="true"
            className="grid w-full max-w-md shrink-0 grid-cols-5 gap-2 lg:w-[42%]"
          >
            {Array.from({ length: 5 }, (_, index) => (
              // A fixed row of five, which is the rotation's own cap (`hero-rotation.ts`): the index
              // is a stable identity for a placeholder that never reorders.
              <Skeleton className="aspect-video w-full" key={index} />
            ))}
          </div>
        </div>
      </div>
    </section>
  );
}
