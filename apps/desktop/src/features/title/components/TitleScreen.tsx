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

import { Button, EmptyState, ErrorState, Screen, Section, Table } from "@peregrine/ui";
import { normalizeIpcError, playExternally, scheduleDownload } from "@peregrine/ipc-client";
import type { IpcError, LibraryTitleDto } from "@peregrine/ipc-client";
import { useState, type ReactElement } from "react";
import { useTranslation } from "react-i18next";
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
import type { Route, TitleKind } from "../../../app/routes.js";
import type { DownloadMarks } from "../../../lib/download-status.js";

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
  watchlisted = false,
  onWatchlist,
  onBack,
  backLabel,
}: TitleScreenProps): ReactElement {
  const { t } = useTranslation();
  const title: LibraryTitleDto | null = detail.title;
  // The external player is a process launch rather than a route, so its refusal is this screen's to
  // say: a Play that opens nothing and explains nothing is the inert control PG-E1-07 forbids.
  const [playFailure, setPlayFailure] = useState<IpcError | null>(null);
  const [scheduled, setScheduled] = useState<ReadonlySet<string>>(() => new Set());
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

  return (
    <Screen backLabel={backLabel} onBack={onBack} title={title?.name ?? kindLabel}>
      <Section description={kindLabel} title={t("title.section")}>
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
                onNavigate({ kind: "player", id: title.id });
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
            />
            {playFailure === null ? null : (
              <p className="text-caption text-state-danger" role="status">
                {messageOf(playFailure)}
              </p>
            )}
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
            {/* The library's own arithmetic stays a table: four counts are a row of numbers rather than
                something a hero draws, and a reader comparing them across pages wants them aligned. */}
            <Table
              columns={[
                {
                  id: "items",
                  header: t("title.column.items"),
                  numeric: true,
                  cell: () => String(title.items),
                },
                {
                  id: "files",
                  header: t("library.column.files"),
                  numeric: true,
                  cell: () => String(title.files),
                },
                {
                  id: "size",
                  header: t("library.column.size"),
                  numeric: true,
                  cell: () => sizeLabel(title.bytes, t),
                },
                {
                  id: "missing",
                  header: t("library.column.missing"),
                  numeric: true,
                  cell: () => (title.missingFiles === 0 ? "—" : String(title.missingFiles)),
                },
              ]}
              rowKey={() => "totals"}
              rows={[title]}
            />
          </div>
        )}
      </Section>

      {/* A film has no season section, and a show whose provider named no seasons draws none either:
          a heading over an empty switcher would be a promise nobody made. */}
      {title === null || title.kind === "movies" || seasons.seasons.length === 0 ? null : (
        <Section description={t("title.episodes.description")} title={t("title.episodes.title")}>
          <div className="flex flex-col gap-4">
            <SeasonSwitcher
              current={seasons.season?.seasonNumber ?? 0}
              onChoose={seasons.choose}
              seasons={seasons.seasons}
            />

            {seasons.failed === null ? null : (
              <p className="text-caption text-state-danger" role="status">
                {t("title.episodes.failed")}
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
            <ul
              className="grid auto-rows-fr list-none gap-3"
              style={{ gridTemplateColumns: "repeat(auto-fill, minmax(18rem, 1fr))" }}
            >
              {seasons.episodes.map((episode) => {
                const nextUp = seasons.episodes.find((row) => !row.watched);
                return (
                  // The season and the number together, because an episode's number is only unique
                  // within its season — and a title can hold the same number in two of them.
                  <li
                    className="h-full"
                    key={`${String(episode.seasonNumber)}-${String(episode.episodeNumber)}`}
                  >
                    <EpisodeCard
                      downloadPercent={
                        acquire?.titleId === title.id ? Math.floor(acquire.permille / 10) : 0
                      }
                      downloading={
                        acquire?.titleId === title.id &&
                        acquire.episode?.season === episode.seasonNumber &&
                        acquire.episode?.episode === episode.episodeNumber &&
                        ((acquire.starting ?? false) || (acquire.running ?? false))
                      }
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

      <TitleFeedList downloads={downloads} feed={feed} onNavigate={onNavigate} />
    </Screen>
  );
}
