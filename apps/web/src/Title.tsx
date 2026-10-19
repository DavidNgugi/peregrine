import { useEffect, useMemo, useRef, useState } from "react";

import { api, type Episode, type PlaybackTicket, type Tile, type Title } from "./api.js";
import { TitleFeed } from "./Feed.js";
import { ChevronLeftIcon } from "./icons.js";
import { useIsMobile } from "./useIsMobile.js";

/** One title's page: a hero, the episodes of a series, and More Like This. */
export function TitleScreen({
  id,
  preview,
  canAcquire,
  onBack,
  onPlay,
  onOpen,
}: {
  readonly id: number;
  readonly preview: Tile | undefined;
  readonly canAcquire: boolean;
  readonly onBack: () => void;
  readonly onPlay: (ticket: PlaybackTicket, name: string) => void;
  readonly onOpen: (tile: Tile) => void;
}) {
  const [title, setTitle] = useState<Title | null>(null);
  const [season, setSeason] = useState<number | undefined>(undefined);
  const [loading, setLoading] = useState(true);
  const [message, setMessage] = useState<string | null>(null);
  const [messageError, setMessageError] = useState(false);
  const [busy, setBusy] = useState(false);
  const [episodesOpen, setEpisodesOpen] = useState(true);
  const mobile = useIsMobile();
  const focused = useRef(false);

  // Focus the Play control once the page has a title to draw. A season switch re-reads the
  // episodes, and moving focus back to the top on every switch would be a wrestling match.
  useEffect(() => {
    if (title === null || focused.current) return;
    focused.current = true;
    const target = document.querySelector<HTMLElement>("[data-autofocus]");
    if (target !== null && !target.hasAttribute("disabled")) {
      target.focus();
    } else {
      document.querySelector<HTMLElement>("[data-focus]:not([disabled])")?.focus();
    }
  }, [title]);

  // A provider refresh fills a catalogue series' seasons and episodes after the page has drawn.
  useEffect(() => {
    if (title?.refreshing !== true) return;
    const timer = window.setInterval(() => {
      void api
        .title(id, season)
        .then((found) => setTitle(found))
        .catch(() => {
          // A dropped poll is not a failed refresh; the next tick asks again.
        });
    }, 2000);
    return () => window.clearInterval(timer);
  }, [title?.refreshing, id, season]);

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
  const backdrop = title?.backdrop ?? preview?.backdrop ?? null;
  const overview = title?.overview ?? preview?.overview ?? null;
  const rating = title?.rating ?? preview?.rating ?? null;
  const released = title?.released ?? null;
  const inLibrary = title?.inLibrary ?? false;
  const episodes = title?.episodes ?? [];
  const seasons = title?.seasons ?? [];
  const resume = title?.progress?.positionMs ?? 0;
  const atEnd = (title?.progress?.percent ?? 0) > 95;
  const startAt = atEnd ? 0 : resume;
  const movie = kind !== "show";

  const nextEpisode = useMemo(() => {
    const started = episodes.find(
      (episode) => episode.playable && episode.percent > 0 && episode.percent < 95,
    );
    return started ?? episodes.find((episode) => episode.playable);
  }, [episodes]);
  const canPlay = movie ? title?.playable === true : nextEpisode !== undefined;

  const download = async (): Promise<void> => {
    setBusy(true);
    setMessage(null);
    try {
      await api.acquire(id);
      setMessageError(false);
      setMessage("Download queued. Follow it in Downloads.");
    } catch (caught) {
      setMessageError(true);
      setMessage(caught instanceof Error ? caught.message : "That download could not be queued.");
    } finally {
      setBusy(false);
    }
  };

  const downloadEpisode = async (episode: Episode): Promise<void> => {
    setBusy(true);
    setMessage(null);
    try {
      await api.acquire(id, episode.seasonNumber, episode.episodeNumber);
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
      setBusy(false);
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

  const meta = [
    movie ? "Film" : "Series",
    released === null ? null : released.slice(0, 4),
    rating === null ? null : rating.toFixed(1),
  ]
    .filter((part): part is string => part !== null)
    .join("  ·  ");

  return (
    <main className="screen title-screen">
      <section className="title-hero">
        {backdrop === null ? null : (
          <img alt="" className="hero-art" decoding="async" src={backdrop} />
        )}
        <button
          aria-label="Back"
          className="back-icon"
          data-focus="true"
          onClick={onBack}
          type="button"
        >
          <ChevronLeftIcon />
        </button>
        <div className="title-hero-copy">
          <p className="hero-meta">{meta}</p>
          <h1>{name}</h1>
          {overview === null ? null : <p className="hero-overview">{overview}</p>}
          {title === null ? (
            <p className="dim">Reading...</p>
          ) : canPlay ? (
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
                        ? "Resume S" + nextEpisode.seasonNumber + " E" + nextEpisode.episodeNumber
                        : "Play S" + nextEpisode.seasonNumber + " E" + nextEpisode.episodeNumber}
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
              {canAcquire && (movie || episodes.length === 0) ? (
                <button
                  className="secondary"
                  data-autofocus="true"
                  data-focus="true"
                  disabled={busy}
                  onClick={() => void download()}
                  type="button"
                >
                  {busy ? "Queuing..." : movie ? "Download film" : "Download series"}
                </button>
              ) : null}
            </div>
          )}
          {message === null ? null : (
            <p aria-live="polite" className={messageError ? "error" : "notice"}>
              {message}
            </p>
          )}
        </div>
      </section>

      <div className="title-body">
        {seasons.length > 1 ? (
          <div className="seasons" role="group">
            {seasons.map((held) => (
              <button
                className={
                  held.number === (season ?? seasons[0]?.number) ? "season current" : "season"
                }
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

        {episodes.length === 0 ? null : (
          <section className="episodes">
            <div className="section-head">
              <h2>Episodes</h2>
              {mobile && episodes.length > 0 ? (
                <button
                  aria-expanded={episodesOpen}
                  className="collapse"
                  data-focus="true"
                  onClick={() => setEpisodesOpen((held) => !held)}
                  type="button"
                >
                  {episodesOpen ? "Hide" : "Show"}
                </button>
              ) : null}
            </div>
            {mobile && !episodesOpen ? null : (
              <div className="episodes-grid">
                {episodes.map((episode) => (
                  <div className="episode" key={episode.seasonNumber + ":" + episode.episodeNumber}>
                    <button
                      className="episode-play"
                      data-focus="true"
                      disabled={!episode.playable}
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
                        {!episode.playable
                          ? "Not downloaded"
                          : episode.watched
                            ? "Play again"
                            : episode.percent > 0
                              ? "Resume"
                              : "Play"}
                      </span>
                    </button>
                    {canAcquire && !episode.playable ? (
                      <button
                        className="episode-download"
                        data-focus="true"
                        disabled={busy}
                        onClick={() => void downloadEpisode(episode)}
                        type="button"
                      >
                        {busy ? "Queuing..." : "Download"}
                      </button>
                    ) : null}
                  </div>
                ))}
              </div>
            )}
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
