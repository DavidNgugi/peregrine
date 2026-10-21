import { useEffect, useMemo, useRef, useState } from "react";

import { api, type Episode, type PlaybackTicket, type Tile, type Title } from "./api.js";
import { downloadTone } from "./download-state.js";
import { TitleFeed } from "./Feed.js";
import { BookmarkIcon, StarIcon } from "./icons.js";
import {
  downloadAction,
  seasonAcquireMessage,
  seasonDownloadAction,
  shownDownloadState,
} from "./title-actions.js";

/** One title's page: a hero, the episodes of a series, and More Like This. */
export function TitleScreen({
  id,
  preview,
  canAcquire,
  onPlay,
  onOpen,
}: {
  readonly id: number;
  readonly preview: Tile | undefined;
  readonly canAcquire: boolean;
  readonly onPlay: (ticket: PlaybackTicket, name: string) => void;
  readonly onOpen: (tile: Tile) => void;
}) {
  const [title, setTitle] = useState<Title | null>(null);
  const [season, setSeason] = useState<number | undefined>(undefined);
  const [loading, setLoading] = useState(true);
  const [message, setMessage] = useState<string | null>(null);
  const [messageError, setMessageError] = useState(false);
  const [busy, setBusy] = useState(false);
  const [pending, setPending] = useState<string | null>(null);
  const [marking, setMarking] = useState<string | null>(null);
  const [watchlistBusy, setWatchlistBusy] = useState(false);
  // A queue action is not instantly visible in the title read: the scheduler inserts the job,
  // then the page's next read may land before its state settles. Keep polling for a bounded
  // window and keep the button in a queued state until the server confirms what happened.
  const [watchingDownload, setWatchingDownload] = useState(false);
  const [episodesOpen, setEpisodesOpen] = useState(true);
  const [seasonsOpen, setSeasonsOpen] = useState(true);
  const focused = useRef(false);

  // Focus the Play control once the page has a title to draw. A season switch re-reads the
  // episodes, and moving focus back to the top on every switch would be a wrestling match.
  useEffect(() => {
    if (title === null || focused.current) return;
    focused.current = true;
    const target = document.querySelector<HTMLElement>("[data-autofocus]");
    // Focus without moving the page: the shell has just scrolled this route to the top, and a
    // focus call that also scrolls would fight it on a short viewport.
    if (target !== null && !target.hasAttribute("disabled")) {
      target.focus({ preventScroll: true });
    } else {
      document
        .querySelector<HTMLElement>("[data-focus]:not([disabled])")
        ?.focus({ preventScroll: true });
    }
  }, [title]);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    void api
      .title(id, season)
      .then((found) => {
        if (cancelled) return;
        setTitle(found);
        setLoading(false);
      })
      .catch((caught: unknown) => {
        if (cancelled) return;
        setMessageError(true);
        setMessage(caught instanceof Error ? caught.message : "That title could not be read.");
        setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [id, season]);

  const name = title?.name ?? preview?.name ?? "...";
  const kind = title?.kind ?? preview?.kind ?? "movie";
  const poster = title?.poster ?? preview?.poster ?? null;
  const backdrop = title?.backdrop ?? preview?.backdrop ?? null;
  const heroArt = backdrop ?? poster;
  const overview = title?.overview ?? preview?.overview ?? null;
  const rating = title?.rating ?? preview?.rating ?? null;
  const released = title?.released ?? null;
  const inLibrary = title?.inLibrary ?? false;
  // The same UTC day the core compares against, so a control and a refusal cannot disagree.
  const today = new Date().toISOString().slice(0, 10);
  const upcomingTitle = released !== null && released > today;
  const episodes = title?.episodes ?? [];
  const seasons = title?.seasons ?? [];
  const resume = title?.progress?.positionMs ?? 0;
  const atEnd = (title?.progress?.percent ?? 0) > 95;
  const startAt = atEnd ? 0 : resume;
  const movie = kind !== "show";
  const selectedSeason = season ?? seasons[0]?.number ?? episodes[0]?.seasonNumber;
  const seasonWatched = episodes.length > 0 && episodes.every((episode) => episode.watched);
  const seasonUpcoming = episodes.some(
    (episode) => episode.airDate !== null && episode.airDate > today,
  );
  const showSeasons = !movie && (seasons.length > 0 || episodes.length > 0);

  const nextEpisode = useMemo(() => {
    const started = episodes.find(
      (episode) =>
        (episode.playable || episode.streamable) && episode.percent > 0 && episode.percent < 95,
    );
    return started ?? episodes.find((episode) => episode.playable || episode.streamable);
  }, [episodes]);
  const canPlay = movie ? title?.playable === true : nextEpisode !== undefined;

  const titleDownload =
    title === null
      ? null
      : downloadAction(
          shownDownloadState(title.downloadState, watchingDownload),
          title.downloadPercent ?? (watchingDownload ? 0 : null),
          movie ? "Download" : "Download series",
        );
  const titleDownloadKey = movie ? "title" : "series";
  const seasonAction =
    selectedSeason === undefined
      ? null
      : seasonDownloadAction(episodes, selectedSeason, watchingDownload);
  const heroDownload =
    !canPlay &&
    canAcquire &&
    (movie || episodes.length === 0) &&
    titleDownload !== null &&
    !titleDownload.disabled;

  // A job that is still moving has to be re-read or its button freezes at the state the click left
  // it in. The read keeps the selected season, and it only replaces the title, never the scroll or
  // the focused control.
  const live =
    watchingDownload ||
    title?.refreshing === true ||
    downloadTone(title?.downloadState) === "queued" ||
    downloadTone(title?.downloadState) === "active" ||
    (title?.episodes.some((episode) => {
      const tone = downloadTone(episode.downloadState);
      return tone === "queued" || tone === "active";
    }) ??
      false);

  // A queue that never becomes visible must not poll forever. Two minutes is longer than any
  // acquisition's metadata search and short enough that a dead job stops costing requests.
  useEffect(() => {
    if (!watchingDownload) return;
    const timer = window.setTimeout(() => setWatchingDownload(false), 120_000);
    return () => window.clearTimeout(timer);
  }, [watchingDownload]);

  useEffect(() => {
    if (!live) return;
    let cancelled = false;
    const timer = window.setInterval(() => {
      void api
        .title(id, season)
        .then((found) => {
          if (!cancelled) setTitle(found);
        })
        .catch(() => {
          // A dropped poll is not a failed action; the next tick asks again.
        });
    }, 2000);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [live, id, season]);

  const patch = (change: (held: Title) => Title): void => {
    setTitle((held) => (held === null ? null : change(held)));
  };

  const setEpisodeWatchedState = (
    seasonNumber: number,
    episodeNumber: number,
    watched: boolean,
  ): void => {
    patch((held) => ({
      ...held,
      episodes: held.episodes.map((episode) =>
        episode.seasonNumber === seasonNumber && episode.episodeNumber === episodeNumber
          ? { ...episode, watched }
          : episode,
      ),
    }));
  };

  const toggleWatchlist = async (): Promise<void> => {
    if (title === null || watchlistBusy) return;
    const next = !title.watchlisted;
    setWatchlistBusy(true);
    setMessage(null);
    patch((held) => ({ ...held, watchlisted: next }));
    try {
      await api.watchlistSet(id, next);
      setMessageError(false);
      setMessage(next ? "Added to My List." : "Removed from My List.");
    } catch (caught) {
      patch((held) => ({ ...held, watchlisted: !next }));
      setMessageError(true);
      setMessage(caught instanceof Error ? caught.message : "My List could not be updated.");
    } finally {
      setWatchlistBusy(false);
    }
  };

  const toggleTitleWatched = async (): Promise<void> => {
    if (title === null || marking !== null) return;
    const next = !title.watched;
    setMarking("title");
    setMessage(null);
    patch((held) => ({ ...held, watched: next }));
    try {
      await api.watchedSet({ titleId: id, scope: "title", watched: next });
      setMessageError(false);
      setMessage(next ? "Marked as watched." : "Marked as unwatched.");
    } catch (caught) {
      patch((held) => ({ ...held, watched: !next }));
      setMessageError(true);
      setMessage(caught instanceof Error ? caught.message : "The watch mark could not be saved.");
    } finally {
      setMarking(null);
    }
  };

  const toggleSeasonWatched = async (seasonNumber: number): Promise<void> => {
    if (marking !== null || episodes.length === 0) return;
    const next = !episodes.every((episode) => episode.watched);
    const previous = new Map(
      episodes.map((episode): [string, boolean] => [
        episodeKey(episode.seasonNumber, episode.episodeNumber),
        episode.watched,
      ]),
    );
    setMarking("season");
    setMessage(null);
    patch((held) => ({
      ...held,
      episodes: held.episodes.map((episode) => ({ ...episode, watched: next })),
    }));
    try {
      await api.watchedSet({ titleId: id, scope: "season", season: seasonNumber, watched: next });
      setMessageError(false);
      setMessage(next ? "Season marked as watched." : "Season marked as unwatched.");
    } catch (caught) {
      patch((held) => ({
        ...held,
        episodes: held.episodes.map((episode) => {
          const before = previous.get(episodeKey(episode.seasonNumber, episode.episodeNumber));
          return before === undefined ? episode : { ...episode, watched: before };
        }),
      }));
      setMessageError(true);
      setMessage(caught instanceof Error ? caught.message : "The season could not be marked.");
    } finally {
      setMarking(null);
    }
  };

  const toggleEpisodeWatched = async (episode: Episode): Promise<void> => {
    if (marking !== null) return;
    const next = !episode.watched;
    setMarking(episodeKey(episode.seasonNumber, episode.episodeNumber));
    setMessage(null);
    setEpisodeWatchedState(episode.seasonNumber, episode.episodeNumber, next);
    try {
      await api.watchedSet({
        titleId: id,
        scope: "episode",
        season: episode.seasonNumber,
        episode: episode.episodeNumber,
        watched: next,
      });
      setMessageError(false);
      setMessage(next ? "Episode marked as watched." : "Episode marked as unwatched.");
    } catch (caught) {
      setEpisodeWatchedState(episode.seasonNumber, episode.episodeNumber, episode.watched);
      setMessageError(true);
      setMessage(caught instanceof Error ? caught.message : "The episode could not be marked.");
    } finally {
      setMarking(null);
    }
  };

  const downloadTitle = async (): Promise<void> => {
    if (pending !== null) return;
    setPending(titleDownloadKey);
    setMessage(null);
    try {
      await api.acquire(id);
      setWatchingDownload(true);
      patch((held) => ({ ...held, downloadState: "queued", downloadPercent: 0 }));
      setMessageError(false);
      setMessage("Download queued. Follow it in Downloads.");
    } catch (caught) {
      setMessageError(true);
      setMessage(caught instanceof Error ? caught.message : "That download could not be queued.");
    } finally {
      setPending(null);
    }
  };

  const downloadSeason = async (seasonNumber: number): Promise<void> => {
    if (pending !== null) return;
    setPending("season");
    setMessage(null);
    try {
      const result = await api.acquireSeason(id, seasonNumber);
      setWatchingDownload(true);
      patch((held) => ({
        ...held,
        episodes: held.episodes.map((episode) => {
          const tone = downloadTone(episode.downloadState);
          // A failed episode is exactly the one a retry is for; only a playable, queued, active,
          // or finished row already has a state a retry would not change.
          return episode.playable ||
            episode.streamable ||
            tone === "queued" ||
            tone === "active" ||
            tone === "done"
            ? episode
            : { ...episode, downloadState: "queued", downloadPercent: 0 };
        }),
      }));
      setMessageError(result.failed > 0);
      setMessage(seasonAcquireMessage(result));
    } catch (caught) {
      setMessageError(true);
      setMessage(caught instanceof Error ? caught.message : "That season could not be queued.");
    } finally {
      setPending(null);
    }
  };

  const scheduleTitle = async (): Promise<void> => {
    if (pending !== null) return;
    setPending("schedule:title");
    setMessage(null);
    try {
      await api.schedule(id);
      patch((held) => ({ ...held, scheduled: true }));
      setMessageError(false);
      setMessage("Scheduled. It will download when it is released.");
    } catch (caught) {
      setMessageError(true);
      setMessage(caught instanceof Error ? caught.message : "That release could not be scheduled.");
    } finally {
      setPending(null);
    }
  };

  const scheduleEpisode = async (episode: Episode): Promise<void> => {
    if (pending !== null) return;
    const key = "schedule:" + episodeKey(episode.seasonNumber, episode.episodeNumber);
    setPending(key);
    setMessage(null);
    try {
      await api.schedule(id, episode.seasonNumber, episode.episodeNumber);
      patch((held) => ({
        ...held,
        episodes: held.episodes.map((heldEpisode) =>
          heldEpisode.seasonNumber === episode.seasonNumber &&
          heldEpisode.episodeNumber === episode.episodeNumber
            ? { ...heldEpisode, scheduled: true }
            : heldEpisode,
        ),
      }));
      setMessageError(false);
      setMessage(
        "Scheduled S" +
          String(episode.seasonNumber) +
          " E" +
          String(episode.episodeNumber) +
          ". It will download when it airs.",
      );
    } catch (caught) {
      setMessageError(true);
      setMessage(caught instanceof Error ? caught.message : "That episode could not be scheduled.");
    } finally {
      setPending(null);
    }
  };

  const downloadEpisode = async (episode: Episode): Promise<void> => {
    if (pending !== null) return;
    const key = episodeKey(episode.seasonNumber, episode.episodeNumber);
    setPending(key);
    setMessage(null);
    try {
      await api.acquire(id, episode.seasonNumber, episode.episodeNumber);
      setWatchingDownload(true);
      patch((held) => ({
        ...held,
        episodes: held.episodes.map((heldEpisode) =>
          episodeKey(heldEpisode.seasonNumber, heldEpisode.episodeNumber) === key
            ? { ...heldEpisode, downloadState: "queued", downloadPercent: 0 }
            : heldEpisode,
        ),
      }));
      setMessageError(false);
      setMessage(
        "Download queued for S" +
          String(episode.seasonNumber) +
          " E" +
          String(episode.episodeNumber) +
          ". Follow it in Downloads.",
      );
    } catch (caught) {
      setMessageError(true);
      setMessage(caught instanceof Error ? caught.message : "That download could not be queued.");
    } finally {
      setPending(null);
    }
  };

  const playMovie = async (positionMs: number): Promise<void> => {
    setBusy(true);
    setMessage(null);
    try {
      const ticket = await api.playback(id, positionMs);
      onPlay(ticket, name);
    } catch (caught) {
      setMessageError(true);
      setMessage(caught instanceof Error ? caught.message : "That film could not be started.");
    } finally {
      setBusy(false);
    }
  };

  const playEpisode = async (episode: Episode, fromStart = false): Promise<void> => {
    setBusy(true);
    setMessage(null);
    try {
      const ticket = await api.playback(
        id,
        fromStart ? 0 : episode.positionMs,
        episode.seasonNumber,
        episode.episodeNumber,
      );
      onPlay(ticket, name);
    } catch (caught) {
      setMessageError(true);
      setMessage(caught instanceof Error ? caught.message : "That episode could not be started.");
    } finally {
      setBusy(false);
    }
  };

  const primary = async (): Promise<void> => {
    if (movie) {
      await playMovie(startAt);
    } else if (nextEpisode !== undefined) {
      await playEpisode(nextEpisode);
    }
  };

  const meta = [movie ? "Film" : "Series", released === null ? null : released.slice(0, 4)]
    .filter((part): part is string => part !== null)
    .join("  ·  ");

  return (
    <main className="screen title-screen">
      <section
        className={backdrop === null && poster !== null ? "title-hero poster-hero" : "title-hero"}
      >
        {heroArt === null ? null : (
          <img
            alt=""
            className={backdrop === null ? "hero-art hero-art-poster" : "hero-art"}
            decoding="async"
            src={heroArt}
          />
        )}
        <div className="title-hero-copy">
          <p className="hero-meta">
            {meta}
            {rating === null ? null : (
              <span className="hero-rating">
                <StarIcon />
                {rating.toFixed(1)}
              </span>
            )}
          </p>
          <h1>{name}</h1>
          {overview === null ? null : <p className="hero-overview">{overview}</p>}
          {title === null ? (
            <p className="dim">Reading...</p>
          ) : (
            <>
              {canPlay ? (
                <div className="actions">
                  <button
                    className="play"
                    data-autofocus="true"
                    data-focus="true"
                    disabled={busy || (movie ? false : nextEpisode === undefined)}
                    onClick={() => void primary()}
                    type="button"
                  >
                    {busy
                      ? "Opening..."
                      : movie
                        ? startAt > 5_000
                          ? "Resume"
                          : "Play"
                        : nextEpisode === undefined
                          ? "No episode on disk"
                          : nextEpisode.percent > 0
                            ? "Resume S" +
                              String(nextEpisode.seasonNumber) +
                              " E" +
                              String(nextEpisode.episodeNumber)
                            : "Play S" +
                              String(nextEpisode.seasonNumber) +
                              " E" +
                              String(nextEpisode.episodeNumber)}
                  </button>
                  {movie && resume > 5_000 && !atEnd ? (
                    <button
                      className="secondary"
                      data-focus="true"
                      disabled={busy}
                      onClick={() => void playMovie(0)}
                      type="button"
                    >
                      Start over
                    </button>
                  ) : null}
                </div>
              ) : (
                <div className="download-cta">
                  <p className="note">
                    {inLibrary
                      ? "No downloaded file is on disk yet."
                      : "This title is not in your library yet."}
                  </p>
                  {canAcquire && (movie || episodes.length === 0) && upcomingTitle ? (
                    <button
                      className="secondary"
                      data-autofocus="true"
                      data-focus="true"
                      disabled={pending !== null || title.scheduled}
                      onClick={() => void scheduleTitle()}
                      type="button"
                    >
                      {pending === "schedule:title"
                        ? "Scheduling..."
                        : title.scheduled
                          ? "Scheduled"
                          : "Schedule"}
                    </button>
                  ) : canAcquire && (movie || episodes.length === 0) && titleDownload !== null ? (
                    <button
                      className="secondary"
                      data-autofocus="true"
                      data-focus="true"
                      disabled={pending !== null || titleDownload.disabled}
                      onClick={() => void downloadTitle()}
                      type="button"
                    >
                      {pending === titleDownloadKey ? "Queuing..." : titleDownload.label}
                    </button>
                  ) : null}
                </div>
              )}
              <div className="actions title-actions">
                <button
                  aria-pressed={title.watchlisted}
                  className="watchlist"
                  data-autofocus={!canPlay && !heroDownload ? "true" : undefined}
                  data-focus="true"
                  disabled={watchlistBusy}
                  onClick={() => void toggleWatchlist()}
                  type="button"
                >
                  <BookmarkIcon />
                  {title.watchlisted ? "In My List" : "My List"}
                </button>
                {movie && !upcomingTitle ? (
                  <button
                    aria-pressed={title.watched}
                    className="watch-toggle"
                    data-focus="true"
                    disabled={marking !== null}
                    onClick={() => void toggleTitleWatched()}
                    type="button"
                  >
                    {title.watched ? "Mark unwatched" : "Mark watched"}
                  </button>
                ) : null}
              </div>
            </>
          )}
          {message === null ? null : (
            <p
              aria-live="polite"
              className={messageError ? "error title-message" : "notice title-message"}
            >
              {message}
            </p>
          )}
        </div>
      </section>

      <div className="title-body">
        {showSeasons ? (
          <section className="seasons-section">
            <div className="section-head seasons-head">
              <h2>Seasons</h2>
              <div className="section-head-actions">
                {episodes.length > 0 &&
                selectedSeason !== undefined &&
                (!seasonUpcoming || seasonWatched) ? (
                  <button
                    aria-pressed={seasonWatched}
                    className="season-watch"
                    data-focus="true"
                    disabled={marking !== null}
                    onClick={() => void toggleSeasonWatched(selectedSeason)}
                    type="button"
                  >
                    {seasonWatched ? "Mark season unwatched" : "Mark season watched"}
                  </button>
                ) : null}
                {seasons.length > 0 ? (
                  <button
                    aria-expanded={seasonsOpen}
                    className="collapse"
                    data-focus="true"
                    onClick={() => setSeasonsOpen((held) => !held)}
                    type="button"
                  >
                    {seasonsOpen ? "Hide" : "Show"}
                  </button>
                ) : null}
              </div>
            </div>
            {seasonsOpen && seasons.length > 0 ? (
              <div aria-label="Seasons" className="seasons" role="group">
                {seasons.map((held) => (
                  <button
                    className={held.number === selectedSeason ? "season current" : "season"}
                    data-focus="true"
                    key={held.number}
                    onClick={() => setSeason(held.number)}
                    type="button"
                  >
                    {held.name ?? "Season " + String(held.number)}
                  </button>
                ))}
              </div>
            ) : null}
          </section>
        ) : null}

        {episodes.length === 0 ? null : (
          <section className="episodes">
            <div className="section-head">
              <h2>Episodes</h2>
              <div className="section-head-actions">
                {canAcquire && selectedSeason !== undefined && seasonAction !== null ? (
                  <button
                    className="season-download"
                    data-focus="true"
                    disabled={pending !== null || seasonAction.disabled}
                    onClick={() => void downloadSeason(selectedSeason)}
                    type="button"
                  >
                    {pending === "season"
                      ? "Queuing season " + String(selectedSeason) + "..."
                      : seasonAction.label}
                  </button>
                ) : null}
                <button
                  aria-expanded={episodesOpen}
                  className="collapse"
                  data-focus="true"
                  onClick={() => setEpisodesOpen((held) => !held)}
                  type="button"
                >
                  {episodesOpen ? "Hide" : "Show"}
                </button>
              </div>
            </div>
            {episodesOpen ? (
              <div className="episodes-grid">
                {episodes.map((episode) => {
                  const key = episodeKey(episode.seasonNumber, episode.episodeNumber);
                  const action = downloadAction(episode.downloadState, episode.downloadPercent);
                  const playable = episode.playable || episode.streamable;
                  // A future episode has nothing to fetch yet; the same rule the desktop's episode
                  // card applies, and the core refuses a future watch mark from either surface.
                  const upcomingEpisode = episode.airDate !== null && episode.airDate > today;
                  return (
                    <div className="episode" key={key}>
                      <button
                        className="episode-play"
                        data-focus="true"
                        disabled={!playable}
                        onClick={() => void playEpisode(episode, episode.watched)}
                        type="button"
                      >
                        <span className="episode-still">
                          {episode.still === null ? null : (
                            <img alt="" decoding="async" loading="lazy" src={episode.still} />
                          )}
                          {episode.percent <= 0 ? null : (
                            <span className="card-progress">
                              <i style={{ width: Math.min(100, episode.percent) + "%" }} />
                            </span>
                          )}
                        </span>
                        <span className="episode-copy">
                          <span className="episode-number">
                            {"S" +
                              String(episode.seasonNumber) +
                              " E" +
                              String(episode.episodeNumber)}
                          </span>
                          <span className="episode-name">
                            {episode.name ?? "Episode " + String(episode.episodeNumber)}
                          </span>
                          <span className="episode-meta">
                            {[
                              episode.runtimeMinutes === null
                                ? null
                                : String(episode.runtimeMinutes) + " min",
                              episode.airDate,
                            ]
                              .filter((part): part is string => part !== null)
                              .join("  ·  ")}
                          </span>
                          {episode.overview === null ? null : (
                            <span className="episode-overview">{episode.overview}</span>
                          )}
                        </span>
                        <span className="episode-action">
                          {!playable
                            ? "Not downloaded"
                            : episode.watched
                              ? "Play again"
                              : episode.percent > 0
                                ? "Resume"
                                : "Play"}
                        </span>
                      </button>
                      <div className="episode-controls">
                        {canAcquire && !playable ? (
                          upcomingEpisode ? (
                            <button
                              className="episode-download"
                              data-focus="true"
                              disabled={pending !== null || episode.scheduled}
                              onClick={() => void scheduleEpisode(episode)}
                              type="button"
                            >
                              {pending === "schedule:" + key
                                ? "Scheduling..."
                                : episode.scheduled
                                  ? "Scheduled"
                                  : "Schedule"}
                            </button>
                          ) : (
                            <button
                              className="episode-download"
                              data-focus="true"
                              disabled={pending !== null || action.disabled}
                              onClick={() => void downloadEpisode(episode)}
                              type="button"
                            >
                              {pending === key ? "Queuing..." : action.label}
                            </button>
                          )
                        ) : null}
                        {upcomingEpisode ? null : (
                          <button
                            aria-pressed={episode.watched}
                            className="episode-watch"
                            data-focus="true"
                            disabled={marking !== null}
                            onClick={() => void toggleEpisodeWatched(episode)}
                            type="button"
                          >
                            {episode.watched ? "Mark unwatched" : "Mark watched"}
                          </button>
                        )}
                      </div>
                    </div>
                  );
                })}
              </div>
            ) : null}
          </section>
        )}

        {title?.refreshing === true && episodes.length === 0 ? (
          <p className="dim episodes-empty">Fetching episodes from the provider...</p>
        ) : null}

        {title !== null &&
        kind === "show" &&
        title.refreshing !== true &&
        episodes.length === 0 &&
        seasons.length === 0 ? (
          <p className="dim episodes-empty">No episodes are listed for this series yet.</p>
        ) : null}

        {episodes.length === 0 && seasons.length > 0 ? (
          <p className="dim episodes-empty">No episodes for this season yet.</p>
        ) : null}

        {title !== null && (title.similar.length > 0 || (title.similarGenres ?? []).length > 0) ? (
          <TitleFeed
            genres={title.similarGenres ?? []}
            kind={kind === "show" ? "show" : "movie"}
            onOpen={onOpen}
            seed={title.similar}
            titleId={id}
          />
        ) : null}

        {loading && title === null ? <p className="dim">Reading...</p> : null}
      </div>
    </main>
  );
}

function episodeKey(seasonNumber: number, episodeNumber: number): string {
  return String(seasonNumber) + ":" + String(episodeNumber);
}
