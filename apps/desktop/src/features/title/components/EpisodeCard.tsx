/**
 * One episode, as a card: its still, its number and name, when it aired, how long it runs, what it is
 * about, and whether the library holds it.
 *
 * **The still is the same promise the poster makes, at a different ratio.** The box is drawn whether or
 * not there is a picture — 16:9, which is what a still is — so a season does not reflow as stills
 * arrive, and a reference the asset protocol refuses lands on the placeholder rather than on a
 * broken-image glyph. `lib/media.ts` is still the only module that names the protocol (AGENTS R11).
 *
 * **The availability chip is a word and not a dot.** `TitleCard` owns the dot's colour mapping, and
 * that map is not exported; copying it here would be the second copy free to drift from the first, which
 * is the defect the card's own doc comment names. So this draws the settled half — the word, which the
 * card's comment calls the half that carries the state — and extracting the chip into a `packages/ui`
 * primitive is recorded as owed rather than half-done here.
 *
 * **The card is not a link.** Playback is `PG-E15`'s and does not exist, and a card that looked
 * clickable and did nothing would be the enabled-control-that-fires-nothing failure the hero's
 * actions exist to avoid. The one control it can draw is the Download button a page passes in, and
 * that button acts on the episode rather than on the card.
 *
 * **The card fills its cell, and a synopsis is a clamped block rather than a document.** A season holds
 * episodes a provider described and episodes it did not, so two cards in one row carry different
 * numbers of facts; `h-full` is what makes them one size, and the clamp is what stops a paragraph long
 * enough to set the height of every card in the grid on its own. The same choice `TitleCard` makes for
 * its prose, for the same reason.
 */

import type { EpisodeDto } from "@peregrine/ipc-client";
import { Button } from "@peregrine/ui";
import { useState, type ReactElement } from "react";
import { useTranslation } from "react-i18next";

import { imageUrl } from "../../../lib/media.js";
import { episodeAvailability } from "../hooks/seasons.js";
import type { SeasonPack } from "../hooks/season-download.js";

/**
 * The intrinsic size the cache's `w300` still variant is served at.
 *
 * Stated so a browser can lay the box out before the bytes arrive, which is the same reason the poster
 * states its own.
 */
const STILL_WIDTH = 300;
const STILL_HEIGHT = 169;

export interface EpisodeCardProps {
  readonly episode: EpisodeDto;
  /**
   * Asks the page to acquire this episode. Omitted where the page has no acquisition control, and
   * then the card draws no button at all rather than a disabled one.
   */
  readonly onDownload?: ((episode: EpisodeDto) => void) | undefined;
  /** Opens the player for this episode, when the library holds a file of it. */
  readonly onPlay?: ((episode: EpisodeDto) => void) | undefined;
  /** This episode's own transfer is in flight, so the button says so rather than offering it again. */
  readonly downloading?: boolean | undefined;
  /**
   * This episode's payload has named a media file the engine can serve.
   *
   * It is what lets Play be offered while `downloading` is true: the transfer is running, but the
   * file is known, and the player reads it from the engine. `false` keeps the Download control and
   * its progress, because a payload with no media entry yet has nothing to play.
   */
  readonly streamable?: boolean | undefined;
  /** How far that transfer has got, 0-100. */
  readonly downloadPercent?: number | undefined;
  /**
   * The season plan's live pack, when one is covering this episode's whole season.
   *
   * The plan has already decided that the pack is live and what it is doing; the card only says
   * so. It must not re-read the queue itself, or the chip and the season control could disagree
   * about the same row.
   */
  readonly seasonPack?: SeasonPack | undefined;
  /** Whether the account finished this episode; the bar under the still fills when it did. */
  readonly watched?: boolean | undefined;
  /** How far through the episode is, as a fraction in `0..=1`, for a partially watched one. */
  readonly progress?: number | undefined;
  /** The first episode the account has not finished, drawn with its own border and word. */
  readonly nextUp?: boolean | undefined;
  /** Schedules an unaired episode instead of offering a download it cannot serve yet. */
  readonly onSchedule?: ((episode: EpisodeDto) => void) | undefined;
  /** Whether this episode is already scheduled. */
  readonly scheduled?: boolean | undefined;
  /** Marks this episode watched, or clears the mark, on the user's authority. */
  readonly onToggleWatched?: ((episode: EpisodeDto, watched: boolean) => void) | undefined;
  /** A watch-state write is in flight, so the control waits rather than sending a second. */
  readonly marking?: boolean | undefined;
}

/** One episode's card. */
export function EpisodeCard({
  episode,
  onDownload,
  onPlay,
  downloading = false,
  downloadPercent,
  seasonPack,
  streamable = false,
  watched = false,
  progress = 0,
  nextUp = false,
  onSchedule,
  scheduled = false,
  onToggleWatched,
  marking = false,
}: EpisodeCardProps): ReactElement {
  const { t } = useTranslation();
  // The reference that failed rather than a flag: moving to another season has to give the next still
  // its own chance, and a boolean would keep the previous season's refusal forever.
  const [failed, setFailed] = useState<string | null>(null);
  const source =
    episode.still !== null && episode.still !== failed ? imageUrl(episode.still) : null;
  // A date in the future is not an availability question: the episode exists in the catalogue and
  // has nothing to fetch until it airs, so it offers no Download and says when it arrives instead.
  const today = new Date().toISOString().slice(0, 10);
  const unaired = episode.airDate !== null && episode.airDate > today;
  const availability = unaired ? null : episodeAvailability(episode);
  // A pack is only the answer for an episode nothing else has claimed: a file on disk keeps its
  // own word, and the pack is what covers the ones still wanted.
  const covering = availability === "wanted" ? seasonPack : undefined;
  const transferring = downloading || covering !== undefined;

  return (
    <article
      className={`flex h-full flex-col overflow-hidden rounded-md border bg-surface-1 transition-colors hover:border-accent focus-within:border-accent ${
        nextUp ? "border-accent ring-1 ring-accent" : "border-border-subtle"
      }`}
    >
      <div className="relative aspect-video w-full shrink-0 bg-surface-2">
        {source === null ? (
          <p className="flex h-full items-center justify-center px-4 text-center text-caption text-text-tertiary">
            {t("title.episode.still.none")}
          </p>
        ) : (
          <img
            alt={t("title.episode.still.alt", {
              number: episode.episodeNumber,
              name: episode.name ?? t("title.unnamedEpisode"),
            })}
            className="h-full w-full object-cover"
            height={STILL_HEIGHT}
            onError={() => {
              setFailed(episode.still);
            }}
            src={source}
            width={STILL_WIDTH}
          />
        )}
        {/* The bar Netflix puts along the bottom of a still: filled means finished, partial means
            part-way. It is aria-hidden because the chip already carries the state in words. */}
        {watched || progress > 0 ? (
          <div aria-hidden="true" className="absolute inset-x-0 bottom-0 h-1 bg-black/55">
            <div
              className="h-full bg-accent"
              style={{ width: `${String(watched ? 100 : Math.round(progress * 100))}%` }}
            />
          </div>
        ) : null}
      </div>

      <div className="flex flex-1 flex-col gap-2 p-3">
        <div className="flex items-baseline justify-between gap-2">
          <h3 className="text-body text-text-primary">
            <span className="text-text-tertiary tabular-nums">
              {`${String(episode.seasonNumber)}×${String(episode.episodeNumber).padStart(2, "0")}`}
            </span>{" "}
            {episode.name ?? (
              <span className="text-text-tertiary">{t("title.unnamedEpisode")}</span>
            )}
          </h3>
          {nextUp ? (
            <span className="shrink-0 rounded-full bg-accent px-2 py-0.5 text-caption font-medium text-[var(--text-on-accent)]">
              {t("title.episode.nextUp")}
            </span>
          ) : null}
          <span className="shrink-0 rounded-full border border-border-subtle px-2 py-0.5 text-caption text-text-secondary">
            {covering === undefined
              ? availability === null
                ? t("title.episode.availability.unaired")
                : t(`title.episode.availability.${availability}`)
              : t(
                  covering.status === "active"
                    ? "title.episode.downloadingWithSeason"
                    : "title.episode.queuedWithSeason",
                  { season: covering.season },
                )}
          </span>
        </div>

        {/* The two facts a reader scans for, and each is its own element rather than a joined sentence:
            a date and a runtime are different kinds of answer and a reader compares them across cards,
            which a sentence makes them read. Either may be absent, and an absent one draws nothing at
            all rather than a dash — a placeholder for a fact nobody stated is noise on every card. */}
        <p className="flex flex-wrap items-center gap-x-3 gap-y-1 text-caption text-text-tertiary">
          {episode.airDate === null ? null : (
            <span>
              {unaired
                ? t("title.episode.airs", { date: episode.airDate })
                : t("title.episode.aired", { date: episode.airDate })}
            </span>
          )}
          {episode.runtimeMinutes === null ? null : (
            <span>{t("title.episode.runtime", { minutes: episode.runtimeMinutes })}</span>
          )}
        </p>

        {episode.overview === null ? null : (
          <p className="line-clamp-3 text-caption text-text-secondary">{episode.overview}</p>
        )}

        {/* A card the library already holds offers no Download: the chip says it is here, and a
            second button would be an offer to fetch what is on disk. While this episode's own
            transfer is in flight the button stays, saying so. */}
        {availability === null ? (
          onSchedule === undefined ? null : (
            <div className="mt-auto flex gap-2 pt-1">
              <Button
                disabled={scheduled}
                onClick={() => {
                  onSchedule(episode);
                }}
                size="sm"
                variant="secondary"
              >
                {scheduled ? t("title.episode.scheduled") : t("title.episode.schedule")}
              </Button>
            </div>
          )
        ) : onPlay === undefined &&
          (onDownload === undefined || (availability !== "wanted" && !transferring)) ? null : (
          <div className="mt-auto flex flex-wrap gap-2 pt-1">
            {onPlay !== undefined &&
            (availability === "available" ||
              availability === "partial" ||
              (downloading && streamable)) ? (
              <Button
                aria-label={t("title.episode.playLabel", {
                  number: `${String(episode.seasonNumber)}×${String(episode.episodeNumber).padStart(2, "0")}`,
                  name: episode.name ?? t("title.unnamedEpisode"),
                })}
                onClick={() => {
                  onPlay(episode);
                }}
                size="sm"
                variant="primary"
              >
                {t("title.episode.play")}
              </Button>
            ) : null}
            {onDownload === undefined || (availability !== "wanted" && !transferring) ? null : (
              <Button
                aria-label={t("title.episode.downloadLabel", {
                  number: `${String(episode.seasonNumber)}×${String(episode.episodeNumber).padStart(2, "0")}`,
                  name: episode.name ?? t("title.unnamedEpisode"),
                })}
                disabled={transferring}
                onClick={() => {
                  onDownload(episode);
                }}
                size="sm"
                variant={transferring ? "primary" : "secondary"}
              >
                {downloading
                  ? downloadPercent === undefined
                    ? t("title.episode.downloadingWaiting")
                    : t("title.episode.downloading", { percent: downloadPercent })
                  : covering === undefined
                    ? t("title.episode.download")
                    : covering.status === "active"
                      ? t("title.episode.downloading", { percent: covering.percent })
                      : t("title.episode.queued")}
              </Button>
            )}
          </div>
        )}

        {/* Offered for what has aired, whatever its availability: marking is a statement about what
            the reader has seen, not about what is on disk. An unaired episode offers Schedule
            instead, and the core refuses a future mark from the web surface too. */}
        {onToggleWatched === undefined || unaired ? null : (
          <div className="flex justify-end pt-1">
            <Button
              aria-label={t(
                watched ? "title.episode.markUnwatchedLabel" : "title.episode.markWatchedLabel",
                {
                  number: `${String(episode.seasonNumber)}×${String(episode.episodeNumber).padStart(2, "0")}`,
                  name: episode.name ?? t("title.unnamedEpisode"),
                },
              )}
              disabled={marking}
              onClick={() => {
                onToggleWatched(episode, !watched);
              }}
              size="sm"
              variant="ghost"
            >
              {watched ? t("title.episode.markUnwatched") : t("title.episode.markWatched")}
            </Button>
          </div>
        )}
      </div>
    </article>
  );
}
