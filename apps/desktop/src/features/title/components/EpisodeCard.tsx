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
  /** This episode's own transfer is in flight, so the button says so rather than offering it again. */
  readonly downloading?: boolean | undefined;
  /** How far that transfer has got, 0-100. */
  readonly downloadPercent?: number | undefined;
}

/** One episode's card. */
export function EpisodeCard({
  episode,
  onDownload,
  downloading = false,
  downloadPercent,
}: EpisodeCardProps): ReactElement {
  const { t } = useTranslation();
  // The reference that failed rather than a flag: moving to another season has to give the next still
  // its own chance, and a boolean would keep the previous season's refusal forever.
  const [failed, setFailed] = useState<string | null>(null);
  const source =
    episode.still !== null && episode.still !== failed ? imageUrl(episode.still) : null;
  const availability = episodeAvailability(episode);

  return (
    <article className="flex h-full flex-col overflow-hidden rounded-md border border-border-subtle bg-surface-1">
      <div className="aspect-video w-full shrink-0 bg-surface-2">
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
          <span className="shrink-0 rounded-full border border-border-subtle px-2 py-0.5 text-caption text-text-secondary">
            {t(`title.episode.availability.${availability}`)}
          </span>
        </div>

        {/* The two facts a reader scans for, and each is its own element rather than a joined sentence:
            a date and a runtime are different kinds of answer and a reader compares them across cards,
            which a sentence makes them read. Either may be absent, and an absent one draws nothing at
            all rather than a dash — a placeholder for a fact nobody stated is noise on every card. */}
        <p className="flex flex-wrap items-center gap-x-3 gap-y-1 text-caption text-text-tertiary">
          {episode.airDate === null ? null : (
            <span>{t("title.episode.aired", { date: episode.airDate })}</span>
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
        {onDownload === undefined || (availability !== "wanted" && !downloading) ? null : (
          <div className="mt-auto pt-1">
            <Button
              aria-label={t("title.episode.downloadLabel", {
                number: `${String(episode.seasonNumber)}×${String(episode.episodeNumber).padStart(2, "0")}`,
                name: episode.name ?? t("title.unnamedEpisode"),
              })}
              disabled={downloading}
              onClick={() => {
                onDownload(episode);
              }}
              size="sm"
              variant={downloading ? "primary" : "secondary"}
            >
              {downloading
                ? t("title.episode.downloading", { percent: downloadPercent ?? 0 })
                : t("title.episode.download")}
            </Button>
          </div>
        )}
      </div>
    </article>
  );
}
