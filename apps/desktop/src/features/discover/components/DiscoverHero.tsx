import { Button, Icon, Link } from "@peregrine/ui";
import { useEffect, useState, type ReactElement } from "react";
import { useTranslation } from "react-i18next";

import type { RailItemDto } from "@peregrine/ipc-client";

import {
  advance,
  currentSlide,
  FIRST_SLIDE,
  heroPlay,
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
  /** The vote count as the caller wrote it, such as "24,000 votes". */
  readonly votesLabel: string | null;
  /** The core's answer about whether the library holds a file for this title. */
  readonly playable: boolean;
}

export interface DiscoverHeroProps {
  /** The rotation, already chosen and resolved (`heroSlides` and the caller's own formatting). */
  readonly slides: readonly HeroSlide[];
  /**
   * What Play does, when this build can play anything at all.
   *
   * Omitted by the only caller there is, deliberately: nothing in this build has a playback surface
   * (`PG-E15`), and a control that fired nothing would be the inert button `PG-E1-07` forbids — so
   * its absence is what disables Play with a stated reason rather than leaving it live and empty.
   */
  readonly onPlay?: (slide: HeroSlide) => void;
  readonly onOpen: (slide: HeroSlide) => void;
}

/** The hero, from slides the caller has already resolved. */
export function DiscoverHero({ slides, onPlay, onOpen }: DiscoverHeroProps): ReactElement | null {
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
      className="relative overflow-hidden rounded-md border border-border-subtle bg-surface-2"
    >
      <div className="relative flex aspect-video w-full flex-col justify-end">
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
        <div className="relative flex flex-col gap-3 p-6">
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
            {slide.votesLabel === null ? null : <span>{slide.votesLabel}</span>}
          </p>
          {slide.overview === null ? null : (
            <p className="line-clamp-3 max-w-[60ch] text-body text-text-secondary">
              {slide.overview}
            </p>
          )}
          <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
            <Button
              disabled={play.kind === "unavailable"}
              onClick={() => {
                onPlay?.(slide);
              }}
              variant="primary"
            >
              {t("discover.hero.play")}
            </Button>
            {/* A link rather than a button, because it says where it goes and a context menu can
                offer it; `variant` is what keeps it looking like the control beside it. */}
            <Link
              href={slide.href}
              onNavigate={() => {
                onOpen(slide);
              }}
              variant="secondary"
            >
              {t("card.details")}
            </Link>
            {play.kind === "play" ? null : (
              <p className="text-caption text-text-secondary" role="status">
                {t(play.reason === "noFile" ? "discover.hero.noFile" : "discover.hero.noPlayer")}
              </p>
            )}
          </div>
          {/* A rotation of one has no strip: a row of one control that re-shows the slide already
              up is a control that does nothing, and `PG-E1-07` says absent rather than inert. */}
          {rotationRuns(count) ? (
            <ul
              aria-label={t("discover.hero.strip")}
              className="flex flex-wrap items-center gap-2 pt-1"
            >
              {slides.map((item, index) => {
                const thumb = item.backdrop ?? item.poster;
                return (
                  <li key={item.id}>
                    <button
                      aria-current={index === active}
                      aria-label={t("discover.hero.show", { name: item.name })}
                      className={[
                        "block aspect-video w-24 overflow-hidden rounded-sm border-2",
                        "focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-focus-ring",
                        index === active
                          ? "border-accent opacity-100"
                          : "border-transparent opacity-70 hover:opacity-100",
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
