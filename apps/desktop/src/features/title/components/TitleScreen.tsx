/**
 * One title's own screen: what it is, what the library holds of it, its poster, its seasons, and the
 * endless feed beneath them.
 *
 * The screen is named by the *route* as well as by the read, which is why the kind travels in the
 * path: a film's page can say "Film" before the core has answered, and a page that waited for its
 * read to say anything at all would be the full-page spinner `PG-E7-02` forbids.
 *
 * The poster is what the cache already holds. Filling that cache is the refresh job the hook asks
 * for when the core says one is due, and this screen neither waits for it nor draws anything from it:
 * a picture that arrives late is a picture that arrives late (`ARCHITECTURE §7` rule 2).
 *
 * **What this screen may offer is the core's answer, not its own arithmetic.** The hero's Play and its
 * acquisition control are drawn from `detail.offer` — `peregrine_domain::TitleOffer`, derived from the
 * title's items, their files, and its jobs — rather than from the counts beside them, because a count
 * cannot tell a copy on disk from a copy the library remembers and cannot see.
 *
 * **What is still owed here** is the rest of a title page: the hero expansion and the backdrop
 * cross-fade, cast, provider availability, and what the actions do once they can act — `PG-E7-11`,
 * `PG-E5`, and `PG-E16` for the player itself.
 */

import { Button, EmptyState, ErrorState, Screen, Section, Skeleton } from "@peregrine/ui";
import {
  normalizeIpcError,
  playExternally,
  scheduleDownload,
  setTitleWatched as markTitleWatched,
} from "@peregrine/ipc-client";
import type { DownloadJobDto, IpcError, LibraryTitleDto } from "@peregrine/ipc-client";
import { useEffect, useState, type ReactElement } from "react";
import { useTranslation } from "react-i18next";
import { CastRail } from "./CastRail.js";
import { TitleHero } from "./TitleHero.js";
import { ReleaseOffers } from "./ReleaseOffers.js";
import { EpisodeCard } from "./EpisodeCard.js";
import { SeasonSwitcher } from "./SeasonSwitcher.js";
import { logUiError } from "../../../lib/error-log.js";
import { messageOf } from "../../../lib/ipc-message.js";
import type { AcquireControl } from "../hooks/useAcquire.js";
import type { TitleDetail } from "../hooks/useTitle.js";
import type { TitleSeasons } from "../hooks/useSeasons.js";
import type { TitleFeed } from "../hooks/useTitleFeed.js";
import { TitleFeedList } from "./TitleFeedList.js";
import { WatchProviders } from "./WatchProviders.js";
import { useWatchAvailability } from "../hooks/useWatchAvailability.js";
import type { Route, TitleKind } from "../../../app/routes.js";
import { downloadStatusOf, type DownloadMarks } from "../../../lib/download-status.js";

interface TitleScreenProps {
  /** Which of the two the route says this is, which is what the page can say immediately. */
  readonly kind: TitleKind;
  /** The title and its episodes as the shell read them. */
  readonly detail: TitleDetail;
  /**
   * The titles the provider considers like this one.
   *
   * Empty is both "nothing found" and "nobody to ask", and the page draws no rail for either: a
   * heading over an empty strip would be a promise the provider did not make, and the two are not
   * distinguishable by the time they reach here.
   */
  /**
   * The endless feed under the episodes: the recommendations, then the catalogue's own pages of the
   * genres they share. One list and one scroll area, where the first shape was a rail with a shelf
   * per genre beneath it.
   */
  readonly feed: TitleFeed;
  /**
   * The seasons and the one on screen, which is a read of its own rather than a slice of the title's:
   * a season's episodes are asked for by season, so a show with six seasons reads one of them.
   */
  readonly seasons: TitleSeasons;
  readonly onNavigate: (route: Route) => void;
  /**
   * The acquisition control: what the Download button does once the core says acquisition is the
   * action left. The page does not decide that — `detail.offer` is the core's answer (R7).
   */
  readonly acquire?: AcquireControl;
  /** A download status per title, forwarded to the feed's tiles. */
  readonly downloads?: DownloadMarks | undefined;
  /** The queue rows, so each episode can read its own transfer rather than one page-wide job. */
  readonly downloadRows?: readonly DownloadJobDto[] | undefined;
  /** Titles on the account's watchlist, forwarded to the feed's tiles. */
  readonly watchlistIds?: ReadonlySet<number> | undefined;
  /** Whether this title is on the watchlist. */
  readonly watchlisted?: boolean | undefined;
  /** Adds or removes this title from the watchlist. */
  readonly onWatchlist?: (() => void) | undefined;
  /** The shell's back control, present only when there is somewhere behind this screen. */
  readonly onBack?: (() => void) | undefined;
  readonly backLabel?: string | undefined;
}

/** What `useTranslation` hands a component. */
type Translate = ReturnType<typeof useTranslation>["t"];

/** Bytes as a row shows them: whole units, the same way the library list does. */
function sizeLabel(bytes: number, t: Translate): string {
  if (bytes === 0) {
    return t("library.size.none");
  }
  const units = ["B", "KB", "MB", "GB", "TB"];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  const rounded = value < 10 ? Math.round(value * 10) / 10 : Math.round(value);
  return t("library.size.value", { value: rounded, unit: units[unit] ?? "B" });
}

/** The title screen. */
export function TitleScreen({
  kind,
  detail,
  feed,
  seasons,
  acquire,
  onNavigate,
  downloads,
  downloadRows,
  watchlistIds,
  watchlisted = false,
  onWatchlist,
  onBack,
  backLabel,
}: TitleScreenProps): ReactElement {
  const { t } = useTranslation();
  const title: LibraryTitleDto | null = detail.title;
  // A projection of the read, not a rule: whether every episode on screen is already marked. One
  // empty season is not "watched", which is why the length test comes first.
  const seasonWatched =
    seasons.episodes.length > 0 && seasons.episodes.every((episode) => episode.watched);
  // Cache-only, and re-read after the refresh this screen already follows. The section hides until
  // a provider has answered, so a title nobody has refreshed does not claim there is nothing to watch.
  const watch = useWatchAvailability(title?.id ?? null, detail.updates + detail.refreshes);
  // The external player is a process launch rather than a route, so its refusal is this screen's to
  // say: a Play that opens nothing and explains nothing is the inert control PG-E1-07 forbids.
  const [playFailure, setPlayFailure] = useState<IpcError | null>(null);
  // A film's whole-title mark, held here so the hero follows the press before the write lands. The
  // read is the core's; this mirrors it, and a failed write puts the mirror back.
  const [titleWatched, setTitleWatched] = useState(detail.watched);
  const [watchMarking, setWatchMarking] = useState(false);
  const [watchMarkFailed, setWatchMarkFailed] = useState(false);
  useEffect(() => {
    setTitleWatched(detail.watched);
  }, [detail.watched]);
  const [scheduled, setScheduled] = useState<ReadonlySet<string>>(() => new Set());
  // **A click is not the queue yet.** The acquire job searches before it creates a download row, and
  // one page-wide acquire state would drop the previous episode's label the moment a second one was
  // clicked. The requested set bridges that gap; the queue row takes over as soon as it exists.
  const [requested, setRequested] = useState<ReadonlySet<string>>(() => new Set());
  const episodeKey = (season: number, episode: number): string =>
    `${String(season)}:${String(episode)}`;
  useEffect(() => {
    const queued = new Set(
      (downloadRows ?? [])
        .filter((job) => job.titleId === title?.id && downloadStatusOf(job.state) === "active")
        .map((job) =>
          job.seasonNumber === null || job.episodeNumber === null
            ? ""
            : episodeKey(job.seasonNumber, job.episodeNumber),
        ),
    );
    setRequested((previous) => {
      const kept = [...previous].filter((key) => !queued.has(key));
      return kept.length === previous.size ? previous : new Set(kept);
    });
  }, [downloadRows, title?.id]);
  useEffect(() => {
    if (acquire?.episode == null || acquire.outcome === null || acquire.outcome === "queued") {
      return;
    }
    const key = episodeKey(acquire.episode.season, acquire.episode.episode);
    setRequested((previous) => {
      if (!previous.has(key)) {
        return previous;
      }
      const next = new Set(previous);
      next.delete(key);
      return next;
    });
  }, [acquire?.episode, acquire?.outcome]);
  // A refused start never creates a row; the bridge must not become a button that lies forever.
  useEffect(() => {
    const timer = setInterval(() => {
      setRequested((previous) => (previous.size === 0 ? previous : new Set()));
    }, 120_000);
    return () => {
      clearInterval(timer);
    };
  }, []);
  const schedule = (
    key: string,
    season: number | null,
    episode: number | null,
    releaseOn: string,
  ): void => {
    if (title === null) return;
    void scheduleDownload(title.id, season, episode, releaseOn)
      .then(() => {
        setScheduled((current) => new Set(current).add(key));
      })
      .catch((caught: unknown) => {
        logUiError("scheduleDownload", normalizeIpcError(caught));
      });
  };

  // The route's kind is the fallback, so the page names itself before the read answers and keeps the
  // same heading after — a heading that changed once the core replied would be a visible flicker.
  const kindLabel =
    title === null
      ? kind === "movie"
        ? t("library.kind.films")
        : t("library.kind.series")
      : title.kind === "movies"
        ? t("library.kind.films")
        : t("library.kind.series");

  // **One number, and it is the only fact the page does not already state.** This used to be a
  // four-column table of items, copies, size, and missing counts, whose common row for a film was
  // `1 · 1 · 1.4 GB · —` — a database readout rather than something a reader can use. Items and
  // copies are counts whose actionable form is a per-file list with qualities (PG-E14-11), and the
  // library screen keeps its copies column for comparing titles across rows. What is left is the
  // question this page can answer today — how much space the title takes — and the one state worth
  // the danger colour: files the last scan could not find.
  const size = title === null || title.bytes === 0 ? null : sizeLabel(title.bytes, t);
  // A title whose every file is missing is the hero's `fileMissing` reason, which already says so;
  // a second sentence beside it would be the same fact twice. The partial case is the one no other
  // surface states, and it is the one that looks present in a listing until it is played.
  const missing =
    title === null || title.missingFiles === 0 || detail.offer === "absent"
      ? null
      : t("title.library.missing", { count: title.missingFiles });

  return (
    <Screen backLabel={backLabel} onBack={onBack} title={title?.name ?? kindLabel}>
      <Section description={size ?? kindLabel} title={t("title.section")}>
        {detail.loading ? null : detail.failed ? (
          <ErrorState body={t("title.unavailable.body")} title={t("title.unavailable.title")} />
        ) : detail.missing || title === null ? (
          <EmptyState body={t("title.missing.body")} size="page" title={t("title.missing.title")} />
        ) : (
          <div className="flex flex-col gap-4">
            <TitleHero
              backdrop={detail.backdrop}
              episodes={seasons.episodes.length}
              kindLabel={kindLabel}
              name={title.name}
              offer={detail.offer}
              // Play opens the player rather than playing here: the video wants the content area, and
              // the title page's counts, seasons, and feed want to stay where they are (`ARCHITECTURE`
              // §6's `/player/:id`).
              onPlay={(reader) => {
                if (reader === "external") {
                  setPlayFailure(null);
                  void playExternally(title.id).catch((caught: unknown) => {
                    setPlayFailure(normalizeIpcError(caught));
                  });
                  return;
                }
                onNavigate({ kind: "player", id: title.id, titleKind: kind });
              }}
              acquiring={
                acquire?.titleId === title.id &&
                ((acquire?.starting ?? false) || (acquire?.running ?? false))
              }
              acquiringPercent={
                acquire?.titleId === title.id ? Math.floor(acquire.permille / 10) : 0
              }
              scheduled={scheduled.has("t")}
              onWatchlist={onWatchlist}
              watchlisted={watchlisted}
              watched={titleWatched}
              watchMarkBusy={watchMarking}
              onToggleWatched={
                kind === "movie" && title !== null
                  ? () => {
                      if (watchMarking) {
                        return;
                      }
                      const next = !titleWatched;
                      setTitleWatched(next);
                      setWatchMarking(true);
                      setWatchMarkFailed(false);
                      void markTitleWatched(title.id, next)
                        .catch((caught: unknown) => {
                          setTitleWatched(!next);
                          setWatchMarkFailed(true);
                          logUiError("setTitleWatched", normalizeIpcError(caught));
                        })
                        .finally(() => {
                          setWatchMarking(false);
                        });
                    }
                  : undefined
              }
              {...(detail.released === null
                ? {}
                : {
                    onSchedule: () => {
                      schedule("t", null, null, detail.released as string);
                    },
                  })}
              onAcquire={
                acquire === undefined
                  ? undefined
                  : () => {
                      void acquire.start(title.id);
                    }
              }
              overview={detail.overview}
              playSupport={detail.playSupport}
              poster={detail.poster}
              rating={detail.rating}
              released={detail.released}
              seasons={seasons.seasons.length}
            >
              {/* Answers the same question the actions do, so it sits under them in the hero's column
                  rather than as a section below the poster. */}
              <WatchProviders data={watch.data} />
            </TitleHero>
            {missing === null ? null : (
              <p className="text-caption text-state-danger" role="status">
                {missing}
              </p>
            )}
            {playFailure === null ? null : (
              <p className="text-caption text-state-danger" role="status">
                {messageOf(playFailure)}
              </p>
            )}
            {watchMarkFailed ? (
              <p className="text-caption text-state-danger" role="status">
                {t("title.hero.markFailed")}
              </p>
            ) : null}
            {acquire === undefined ||
            acquire.titleId !== title.id ||
            (acquire.outcome === null && !acquire.running) ? null : (
              <div className="flex items-center gap-3" role="status">
                <div className="flex flex-col gap-1">
                  <p className="text-caption text-text-secondary">
                    {acquire.running
                      ? t("downloads.acquireFinding", {
                          percent: Math.floor(acquire.permille / 10),
                        })
                      : acquire.outcome === "queued"
                        ? t("downloads.acquireQueued")
                        : acquire.outcome === "blocked"
                          ? t("downloads.acquireBlocked")
                          : (acquire.failure ?? t("downloads.acquireFailed"))}
                  </p>
                  {acquire.outcome === "blocked" && acquire.findings.length > 0 ? (
                    <ul className="flex flex-col gap-1 text-caption text-text-tertiary">
                      {acquire.findings.map((finding, index) => (
                        <li key={`${finding.reason}-${String(index)}`}>
                          {t(`downloads.finding.${finding.reason}`, {
                            defaultValue: finding.reason,
                          })}
                          {finding.entry === null ? "" : ` — ${finding.entry}`}
                        </li>
                      ))}
                    </ul>
                  ) : null}
                </div>
                <Button
                  onClick={() => {
                    onNavigate({ kind: "downloads" });
                  }}
                  size="sm"
                  variant="secondary"
                >
                  {t("downloads.view")}
                </Button>
              </div>
            )}
            {acquire === undefined ||
            acquire.titleId !== title.id ||
            acquire.error === null ? null : (
              <p className="text-caption text-state-danger" role="status">
                {messageOf(acquire.error)}
              </p>
            )}
            {/* A preferred quality that is missing is a choice, not a failure: the core keeps what a
                search did find, and this offers them with a Download each. It reloads when the
                acquire control settles, because that is when a search persists any offers. */}
            <ReleaseOffers
              onNavigate={onNavigate}
              reloadKey={acquire?.titleId === title.id ? (acquire?.outcome ?? null) : null}
              titleId={title.id}
            />
          </div>
        )}
      </Section>

      {/* **The page says what it is waiting for.** The refresh is several requests — details,
          credits, every season's episodes, then the pictures — and each part is drawn as it lands;
          this line is the one place the fact that more is coming is said in words, because a
          skeleton is `aria-hidden` and a screen reader should not be told the page is empty. */}
      {detail.refreshing ? (
        <p className="text-body text-text-secondary" role="status">
          {t("title.refreshing")}
        </p>
      ) : null}

      <CastRail
        cast={detail.cast}
        loading={detail.refreshing && detail.cast.length === 0}
        onPerson={(id) => {
          onNavigate({ kind: "person", id });
        }}
      />

      {title?.kind === "shows" && seasons.seasons.length === 0 && detail.refreshing ? (
        <Section description={t("title.episodes.description")} title={t("title.episodes.title")}>
          <div className="flex flex-col gap-4">
            <Skeleton className="h-9 w-64 rounded-full" />
            <div
              className="grid auto-rows-fr gap-3"
              style={{ gridTemplateColumns: "repeat(auto-fill, minmax(18rem, 1fr))" }}
            >
              {Array.from({ length: 6 }, (_, index) => (
                <Skeleton className="h-48" key={index} />
              ))}
            </div>
          </div>
        </Section>
      ) : null}

      {title === null || title.kind === "movies" || seasons.seasons.length === 0 ? null : (
        <Section description={t("title.episodes.description")} title={t("title.episodes.title")}>
          <div className="flex flex-col gap-4">
            <SeasonSwitcher
              current={seasons.season?.seasonNumber ?? 0}
              onChoose={seasons.choose}
              seasons={seasons.seasons}
            />

            {seasons.episodes.length === 0 ? null : (
              <div className="flex flex-wrap items-center justify-end gap-2">
                <Button
                  disabled={seasons.marking}
                  onClick={() => {
                    void seasons.setSeasonWatched(!seasonWatched);
                  }}
                  size="sm"
                  variant="secondary"
                >
                  {seasonWatched
                    ? t("title.episodes.markSeasonUnwatched")
                    : t("title.episodes.markSeasonWatched")}
                </Button>
              </div>
            )}

            {seasons.failed === null ? null : (
              <p className="text-caption text-state-danger" role="status">
                {t("title.episodes.failed")}
              </p>
            )}

            {seasons.markFailed === null ? null : (
              <p className="text-caption text-state-danger" role="status">
                {t("title.episodes.markFailed")}
              </p>
            )}

            {/* A grid rather than a list of rows: a card carries a still, and a still in a table cell
                is a picture the table's own rhythm was never built for. `auto-fill` at a card's width
                is what reflows it with the window.

                **`auto-rows-fr` because the cards do not carry the same number of facts.** A provider
                dates and describes some episodes and not others, so a card with a date, a runtime, and
                a synopsis sits beside one with none — and rows sized to their own content stagger, which
                reads as a broken grid rather than as missing metadata. Equal rows plus a card that fills
                its cell (`EpisodeCard`'s `h-full`) makes every card in a season exactly one size. */}
            {seasons.loading && seasons.episodes.length === 0 ? (
              <div
                className="grid auto-rows-fr gap-3"
                style={{ gridTemplateColumns: "repeat(auto-fill, minmax(18rem, 1fr))" }}
              >
                {Array.from({ length: 6 }, (_, index) => (
                  <Skeleton className="h-48" key={index} />
                ))}
              </div>
            ) : (
              <ul
                className="grid auto-rows-fr list-none gap-3"
                style={{ gridTemplateColumns: "repeat(auto-fill, minmax(18rem, 1fr))" }}
              >
                {seasons.episodes.map((episode) => {
                  const nextUp = seasons.episodes.find((row) => !row.watched);
                  const key = episodeKey(episode.seasonNumber, episode.episodeNumber);
                  const job = (downloadRows ?? []).find(
                    (row) =>
                      row.titleId === title.id &&
                      row.seasonNumber === episode.seasonNumber &&
                      row.episodeNumber === episode.episodeNumber &&
                      downloadStatusOf(row.state) === "active",
                  );
                  return (
                    // The season and the number together, because an episode's number is only unique
                    // within its season — and a title can hold the same number in two of them.
                    <li
                      className="h-full"
                      key={`${String(episode.seasonNumber)}-${String(episode.episodeNumber)}`}
                    >
                      <EpisodeCard
                        {...(job === undefined
                          ? {}
                          : { downloadPercent: Math.round(job.progressPermille / 10) })}
                        downloading={job !== undefined || requested.has(key)}
                        episode={episode}
                        onSchedule={
                          episode.airDate === null
                            ? undefined
                            : (chosen) => {
                                schedule(
                                  "e" +
                                    String(chosen.seasonNumber) +
                                    "-" +
                                    String(chosen.episodeNumber),
                                  chosen.seasonNumber,
                                  chosen.episodeNumber,
                                  chosen.airDate ?? "",
                                );
                              }
                        }
                        scheduled={scheduled.has(
                          "e" + String(episode.seasonNumber) + "-" + String(episode.episodeNumber),
                        )}
                        nextUp={
                          nextUp?.seasonNumber === episode.seasonNumber &&
                          nextUp?.episodeNumber === episode.episodeNumber
                        }
                        progress={episode.progress ?? 0}
                        watched={episode.watched}
                        marking={seasons.marking}
                        onToggleWatched={(chosen, watched) => {
                          void seasons.setEpisodeWatched(chosen, watched);
                        }}
                        onPlay={(chosen) => {
                          onNavigate({
                            kind: "player",
                            id: title.id,
                            season: chosen.seasonNumber,
                            episode: chosen.episodeNumber,
                          });
                        }}
                        onDownload={
                          acquire === undefined
                            ? undefined
                            : (chosen) => {
                                setRequested((previous) =>
                                  new Set(previous).add(
                                    episodeKey(chosen.seasonNumber, chosen.episodeNumber),
                                  ),
                                );
                                void acquire.start(title.id, {
                                  season: chosen.seasonNumber,
                                  episode: chosen.episodeNumber,
                                });
                              }
                        }
                      />
                    </li>
                  );
                })}
              </ul>
            )}

            {seasons.loading || seasons.episodes.length > 0 || seasons.failed !== null ? null : (
              <EmptyState
                body={t("title.episodes.none.body")}
                size="inline"
                title={t("title.episodes.none.title")}
              />
            )}
          </div>
        </Section>
      )}

      <TitleFeedList
        downloads={downloads}
        feed={feed}
        onNavigate={onNavigate}
        watchlisted={watchlistIds}
      />
    </Screen>
  );
}
