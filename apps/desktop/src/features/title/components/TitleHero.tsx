/**
 * A title's own page top: the wide picture behind the poster, the name, what is known about it, and the
 * actions it can actually honour.
 *
 * **The backdrop is the thing the page was missing, and the scrim is why it needs one.** A provider's
 * wide image is a photograph of whatever the marketing department chose, so white text over it is
 * sometimes unreadable and sometimes invisible — the gradient is not decoration, it is what makes the
 * name legible over an arbitrary picture. It runs from the surface colour at the foot to transparent
 * above it, so the copy sits on the page's own background and the image shows through the top half.
 *
 * **The actions come from the core, and the acquisition control is absent rather than disabled when the
 * library already holds the title.** `offer` is `peregrine_domain::TitleOffer`, derived from the
 * title's items, their files, and its jobs; a page that decided this from a count would offer a
 * download of something already on disk. Nothing here is an enabled control that fires nothing: this
 * build has no playback surface (`PG-E16`) and no acquisition surface (`PG-E13`), so Play is disabled
 * with the core's reason beside it whenever it cannot be honoured, and Download is drawn disabled with
 * the same kind of note while acquisition is unbuilt.
 *
 * **The genre chips and the cast column are absent, and that is a decision rather than an omission.**
 * A title's genres live in `title_genre` and its people in `credit`, and **nothing in this build writes
 * either table** — a live installation holds zero rows in both. A read for a table nothing fills would
 * be code that can never draw, which `AGENTS R3` forbids; the rows land with the sync that fills them,
 * and `PG-E7-12` owns the cast column. A page that invented a genre list from the provider's
 * recommendations would be worse than one that showed none, because a reader cannot tell an inferred
 * genre from a stated one.
 */

import { Button, Icon } from "@peregrine/ui";
import type { LocalPlaybackDto, TitleOfferDto } from "@peregrine/ipc-client";
import { useState, type ReactElement } from "react";
import { useTranslation } from "react-i18next";

import { imageUrl } from "../../../lib/media.js";
import { titlePageActions, type PlayReason } from "../../../lib/play-action.js";
import { TitleArtwork } from "./TitleArtwork.js";

/** The aspect a provider's backdrop is served at, so the box is laid out before the bytes arrive. */
const BACKDROP_WIDTH = 780;
const BACKDROP_HEIGHT = 439;

/** The catalogue key of the sentence each reason is said with. */
const REASON_COPY: Readonly<Record<PlayReason, string>> = {
  noFile: "title.hero.noFile",
  fileMissing: "title.hero.fileMissing",
  arriving: "title.hero.arriving",
  noPlayer: "title.hero.noPlayer",
};

export interface TitleHeroProps {
  readonly name: string;
  /** Film or series, which the route already knows and the read confirms. */
  readonly kindLabel: string;
  readonly poster: string | null;
  readonly backdrop: string | null;
  /** When it came out, as the core rendered the stored column, or `null` when nobody stated one. */
  readonly released: string | null;
  /** How many seasons the read found, which is zero for a film and for a show nobody has described. */
  readonly seasons: number;
  /** How many episodes the library holds, which is the count the switcher's seasons add up to. */
  readonly episodes: number;
  readonly rating: number | null;
  readonly overview: string | null;
  /**
   * What this title's page may offer, as the core derived it: whether a copy is on disk, whether one
   * is arriving, whether the library has a row for one it cannot see, or whether acquisition is the
   * action left.
   */
  readonly offer: TitleOfferDto;
  /**
   * What this build would do with the container of the copy the library holds, as the core classified
   * it, or `null` when it holds no copy on disk.
   */
  readonly playSupport: LocalPlaybackDto | null;
  /**
   * What Play does when it can. Absent means this build has no player to give.
   *
   * The reader is the core's classification rather than a screen's: a container the webview cannot
   * demux opens in mpv, and this is what tells the two apart.
   */
  readonly onPlay?: (reader: "webview" | "external") => void;
  /** What Download does when it can. Absent means this build has no acquisition path to give. */
  readonly onAcquire?: (() => void) | undefined;
  /** A start is in flight, so the control waits rather than offering a second. */
  readonly acquiring?: boolean | undefined;
  /** How far the transfer has got, 0-100, while {@link TitleHeroProps.acquiring} is true. */
  readonly acquiringPercent?: number | undefined;
}

/** The hero. */
export function TitleHero({
  name,
  kindLabel,
  poster,
  backdrop,
  released,
  seasons,
  episodes,
  rating,
  overview,
  offer,
  playSupport,
  onPlay,
  onAcquire,
  acquiring,
  acquiringPercent,
}: TitleHeroProps): ReactElement {
  const { t } = useTranslation();
  // The reference that failed rather than a flag: another title's page has to give the next backdrop
  // its own chance, and a boolean would keep this page's refusal forever.
  const [failed, setFailed] = useState<string | null>(null);
  const source = backdrop !== null && backdrop !== failed ? imageUrl(backdrop) : null;
  const actions = titlePageActions(offer, onPlay !== undefined, playSupport);

  return (
    <section className="relative flex flex-col gap-4 overflow-hidden rounded-md border border-border-subtle bg-surface-1">
      {/* The picture is behind everything and clipped by the section's own radius, so a page never
          shows an image escaping its box at a corner. */}
      {source === null ? null : (
        <img
          alt=""
          aria-hidden="true"
          className="pointer-events-none absolute inset-0 h-40 w-full object-cover"
          height={BACKDROP_HEIGHT}
          onError={() => {
            // A refusal from the asset protocol and a blob an OS cleaner took are the same thing to a
            // reader, and both fall back to the page's own surface rather than to a broken-image glyph.
            setFailed(backdrop);
          }}
          src={source}
          width={BACKDROP_WIDTH}
        />
      )}
      {/* The scrim: the surface colour at the foot, transparent above it. Token-driven so both themes
          keep their own contrast rather than a hard-coded black that only works on a dark page. */}
      <div
        aria-hidden="true"
        className="pointer-events-none absolute inset-0 h-40 bg-gradient-to-b from-transparent to-surface-1"
      />

      <div className="relative flex flex-wrap items-start gap-6 p-6">
        <TitleArtwork name={name} poster={poster} />

        <div className="flex min-w-[16rem] flex-1 flex-col gap-3">
          <div className="flex flex-wrap items-center gap-3">
            <h2 className="text-title-1 text-text-primary">{name}</h2>
            {rating === null ? null : (
              <span className="flex items-center gap-1 rounded-full border border-border-subtle px-2 py-0.5 text-caption text-text-secondary">
                <Icon name="star" size={16} />
                {rating.toFixed(1)}
              </span>
            )}
          </div>

          {/* The facts a reader checks before deciding, in the reference's order: what it is, when it
              came out, how much of it there is. Each is its own element rather than a joined sentence,
              because a reader compares them across pages and a sentence makes them read. */}
          <p className="flex flex-wrap items-center gap-x-3 gap-y-1 text-caption text-text-secondary">
            <span>{kindLabel}</span>
            {released === null ? null : <span>{t("title.hero.released", { date: released })}</span>}
            {seasons === 0 ? null : <span>{t("title.hero.seasons", { count: seasons })}</span>}
            {episodes === 0 ? null : <span>{t("title.hero.episodes", { count: episodes })}</span>}
          </p>

          {overview === null ? null : (
            <p className="max-w-prose text-body text-text-secondary">{overview}</p>
          )}

          <div className="flex flex-wrap items-center gap-3">
            <Button
              disabled={actions.play.kind !== "play"}
              onClick={() => {
                if (actions.play.kind === "play") {
                  onPlay?.(actions.play.reader);
                }
              }}
              variant="primary"
            >
              <Icon name="play" size={20} />
              {t("title.hero.play")}
            </Button>
            {/* Drawn only when the core says acquisition is what is left. A title the library holds —
                or one whose transfer is running, or whose file is recorded and absent — gets no
                Download control at all, because a disabled one beside a working Play still reads as an
                offer to fetch what the user already has. */}
            {actions.acquire ? (
              <Button
                disabled={onAcquire === undefined || acquiring === true}
                onClick={onAcquire}
                // The surface has one affirmative action: while Play can run it is Play, and
                // Download is the alternate; when there is nothing to play, Download is it.
                variant={
                  acquiring === true
                    ? "primary"
                    : actions.play.kind === "play"
                      ? "secondary"
                      : "primary"
                }
              >
                {acquiring === true
                  ? t("title.hero.downloading", { percent: acquiringPercent ?? 0 })
                  : t("title.hero.download")}
              </Button>
            ) : null}
            {/* The reason, said rather than left to a disabled control: a reader who does not know why
                a button is inert reads it as a bug. Each reason is the core's own, so the sentence and
                the state cannot disagree.

                **An available action can carry a sentence too, and this is the one case.** A file whose
                container this build cannot name might play or might not — the decoder is the platform's
                and it is not limited to the formats Peregrine can name — so the control stays live and
                the doubt is said beside it. Withholding playback that works and promising playback that
                fails are both worse than a sentence. */}
            {actions.play.kind === "play" ? (
              actions.play.reader === "external" ? (
                <p className="text-caption text-text-secondary" role="status">
                  {t("title.hero.external")}
                </p>
              ) : actions.play.containerUnknown ? (
                <p className="text-caption text-text-secondary" role="status">
                  {t("title.hero.unnamedContainer")}
                </p>
              ) : null
            ) : (
              <p className="text-caption text-text-secondary" role="status">
                {t(REASON_COPY[actions.play.reason])}
              </p>
            )}
          </div>

          {actions.acquire ? (
            <p className="text-caption text-text-tertiary">{t("title.hero.acquire")}</p>
          ) : null}
        </div>
      </div>
    </section>
  );
}
