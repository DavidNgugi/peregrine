/**
 * The player: direct bytes when the browser demuxes the file, HLS when the host transcodes it,
 * and one progress writer either way.
 *
 * The timestamp the person sees is absolute in the title, while the media element's clock starts at
 * whatever offset the transcode was started at. Seeking is therefore a request for a new session at
 * the target offset, not a call to `currentTime`: a transcode cannot seek backwards, and a seek
 * beyond what the encoder has produced would stall. The debounce turns a held arrow key into one
 * restart.
 *
 * **The sidecars are served by the listener, never built here.** Every subtitle URL in the ticket
 * already answers WebVTT and, for an HLS session, already carries the transcode's offset. The
 * element's own `textTracks` are the only source of truth for what is loaded, because a direct
 * file and an hls.js attachment populate them at different times.
 *
 * **Ending is the only autoplay trigger.** The next episode is minted from the `ended` event and
 * only while this screen is still mounted; a pause, a seek, or the back control never starts one.
 */

import type Hls from "hls.js";
import { useCallback, useEffect, useRef, useState, type ReactElement } from "react";

import { api, type PlaybackTicket } from "./api.js";
import { ChevronLeftIcon } from "./icons.js";
import {
  audioOptionIndex,
  initialSubtitleIndex,
  nextEpisodeRequest,
  subtitleLabel,
  totalDurationMs,
} from "./player-tracks.js";

export interface PlayerProps {
  readonly titleId: number;
  readonly titleName: string;
  readonly initial: PlaybackTicket;
  readonly onClose: () => void;
}

/** One rendition in the audio menu. */
interface AudioOption {
  readonly id: string;
  readonly label: string;
  readonly language: string | null;
}

/** Where the audio list came from, and therefore how a choice is applied. */
interface AudioMenu {
  readonly source: "hls" | "native";
  readonly options: readonly AudioOption[];
}

/** Safari's audio track records are not in lib.dom. */
interface NativeAudioTrack {
  readonly id: string;
  readonly label: string;
  readonly language: string;
  enabled: boolean;
}

interface NativeAudioTrackList {
  readonly length: number;
  readonly [index: number]: NativeAudioTrack;
}

function nativeAudioTracks(element: HTMLVideoElement | null): NativeAudioTrackList | null {
  if (element === null) return null;
  const media = element as HTMLVideoElement & {
    readonly audioTracks?: NativeAudioTrackList;
  };
  return media.audioTracks ?? null;
}

/** The track order a subtitle selection is meaningful in; ids are stable within one session. */
function subtitleIds(tracks: PlaybackTicket["subtitles"]): string {
  return tracks.map((track) => String(track.id)).join(",");
}

export function Player({ titleId, titleName, initial, onClose }: PlayerProps): ReactElement {
  const video = useRef<HTMLVideoElement>(null);
  const hls = useRef<Hls | null>(null);
  const [ticket, setTicket] = useState<PlaybackTicket>(initial);
  const [playing, setPlaying] = useState(false);
  const [absoluteMs, setAbsoluteMs] = useState(initial.startPositionMs);
  const [totalMs, setTotalMs] = useState<number | null>(initial.durationMs);
  const [pendingMs, setPendingMs] = useState<number | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  const pending = useRef<number | null>(null);
  const timer = useRef<number | null>(null);
  const reportRef = useRef<(keepalive?: boolean) => void>(() => {});
  const [battery, setBattery] = useState<{
    readonly percent: number;
    readonly charging: boolean;
  } | null>(null);

  const [selectedSubtitle, setSelectedSubtitle] = useState<number | null>(() =>
    initialSubtitleIndex(initial.subtitles),
  );
  const subtitleKey = useRef(subtitleIds(initial.subtitles));
  const [audioMenu, setAudioMenu] = useState<AudioMenu | null>(null);
  const [selectedAudio, setSelectedAudio] = useState<number | null>(null);
  const [startingNext, setStartingNext] = useState<{
    readonly seasonNumber: number;
    readonly episodeNumber: number;
  } | null>(null);
  const [finished, setFinished] = useState(false);

  // A mint can outlive the screen that asked for it; these refs are how a late answer is dropped.
  const mounted = useRef(true);
  const advancing = useRef(false);
  const endedReported = useRef(false);
  const latestTicket = useRef(ticket);
  latestTicket.current = ticket;

  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  // The parent stops the ticket it opened; a ticket this screen minted (a seek restart or the
  // next episode) is not the one the parent knows, so its session is stopped here on the way out.
  useEffect(() => {
    return () => {
      if (latestTicket.current.url !== initial.url) {
        void api.stopPlayback(latestTicket.current.url).catch(() => {});
      }
    };
  }, [initial.url]);

  // The host's charge is what the stream dies with, so the warning belongs on the screen doing
  // the streaming rather than on the computer it is coming from.
  useEffect(() => {
    let cancelled = false;
    const read = (): void => {
      void api
        .host()
        .then((host) => {
          if (!cancelled) setBattery(host.battery);
        })
        .catch(() => {});
    };
    read();
    const interval = window.setInterval(read, 30_000);
    return () => {
      cancelled = true;
      window.clearInterval(interval);
    };
  }, []);

  useEffect(() => {
    document.querySelector<HTMLElement>("[data-autofocus]")?.focus();
  }, []);

  // Attach the chosen delivery. A ticket change is a new session, so the old one is destroyed
  // before the new URL is loaded.
  useEffect(() => {
    const element = video.current;
    if (element === null) return;
    let cancelled = false;
    hls.current?.destroy();
    hls.current = null;
    setMessage(null);
    setTotalMs(ticket.durationMs);
    setAbsoluteMs(ticket.startPositionMs);
    pending.current = null;
    setPendingMs(null);
    setAudioMenu(null);
    setSelectedAudio(null);
    setFinished(false);
    endedReported.current = false;
    if (ticket.kind === "hls") {
      void import("hls.js")
        .then(({ default: HlsPlayer }) => {
          if (cancelled) return;
          if (HlsPlayer.isSupported()) {
            const instance = new HlsPlayer({ enableWorker: true });
            hls.current = instance;
            instance.on(HlsPlayer.Events.ERROR, (_event, data) => {
              if (!data.fatal) return;
              // A segment that was not on disk yet and a decoder hiccup are both recoverable; a
              // message is for the case that is not, not for every retry on the way there.
              if (data.type === HlsPlayer.ErrorTypes.NETWORK_ERROR) {
                instance.startLoad();
                return;
              }
              if (data.type === HlsPlayer.ErrorTypes.MEDIA_ERROR) {
                instance.recoverMediaError();
                return;
              }
              setMessage("The transcode stopped. Choose a position to try again.");
            });
            instance.on(HlsPlayer.Events.FRAG_BUFFERED, () => setMessage(null));
            const readAudio = (): void => {
              if (cancelled) return;
              const tracks = instance.audioTracks;
              if (tracks.length <= 1) {
                setAudioMenu(null);
                setSelectedAudio(null);
                return;
              }
              const options: AudioOption[] = tracks.map((track) => ({
                id: "hls-" + String(track.id),
                label: track.name.trim() || track.lang || "Audio " + String(track.id),
                language: track.lang ?? null,
              }));
              setAudioMenu({ source: "hls", options });
              setSelectedAudio((current) =>
                current !== null && current < options.length
                  ? current
                  : audioOptionIndex(options, ticket.audioLanguage),
              );
            };
            instance.on(HlsPlayer.Events.MANIFEST_PARSED, () => {
              setMessage(null);
              readAudio();
              void element.play().catch(() => {
                setPlaying(false);
              });
            });
            instance.on(HlsPlayer.Events.AUDIO_TRACKS_UPDATED, readAudio);
            instance.attachMedia(element);
            instance.loadSource(ticket.url);
          } else if (element.canPlayType("application/vnd.apple.mpegurl") !== "") {
            element.src = ticket.url;
            void element.play().catch(() => {
              setPlaying(false);
            });
          } else {
            setMessage("This browser cannot play HLS.");
          }
        })
        .catch(() => {
          setMessage("The player could not be loaded.");
        });
    } else {
      element.src = ticket.url;
      element.addEventListener(
        "loadedmetadata",
        () => {
          element.currentTime = ticket.startPositionMs / 1000;
        },
        { once: true },
      );
      void element.play().catch(() => {
        setPlaying(false);
      });
    }
    return () => {
      cancelled = true;
      hls.current?.destroy();
      hls.current = null;
      element.removeAttribute("src");
      element.load();
    };
  }, [ticket]);

  // A seek restart keeps the same subtitle set, so a choice made before the seek survives it; a
  // new episode's set is different and starts from the core's default.
  useEffect(() => {
    const key = subtitleIds(ticket.subtitles);
    if (key === subtitleKey.current) return;
    subtitleKey.current = key;
    setSelectedSubtitle(initialSubtitleIndex(ticket.subtitles));
  }, [ticket]);

  // **The mode is set on the element's own track objects.** A track appears in `textTracks` only
  // once the element has loaded it, and direct load, hls.js attachment, and a new session all
  // reach that point at different times; `addtrack` and `loadedmetadata` are what make the
  // choice outlive those transitions.
  useEffect(() => {
    const element = video.current;
    if (element === null) return;
    const apply = (): void => {
      for (let index = 0; index < element.textTracks.length; index += 1) {
        const entry = element.textTracks[index];
        if (entry === undefined) continue;
        entry.mode = index === selectedSubtitle ? "showing" : "disabled";
      }
    };
    apply();
    element.addEventListener("loadedmetadata", apply);
    const tracks = element.textTracks;
    tracks.addEventListener("addtrack", apply);
    return () => {
      element.removeEventListener("loadedmetadata", apply);
      tracks.removeEventListener("addtrack", apply);
    };
  }, [selectedSubtitle, ticket]);

  // Safari exposes audio renditions on the element; hls.js exposes them on the instance. This
  // reads the native list when metadata arrives and offers nothing for a single rendition.
  useEffect(() => {
    const element = video.current;
    if (element === null) return;
    const read = (): void => {
      // hls.js owns its own rendition list; the element's list is not the same answer.
      if (hls.current !== null) return;
      const list = nativeAudioTracks(element);
      if (list === null || list.length <= 1) {
        setAudioMenu(null);
        setSelectedAudio(null);
        return;
      }
      const options: AudioOption[] = [];
      for (let index = 0; index < list.length; index += 1) {
        const track = list[index];
        if (track === undefined) continue;
        options.push({
          id: "native-" + track.id,
          label: track.label || track.language || "Audio " + String(index),
          language: track.language === "" ? null : track.language,
        });
      }
      setAudioMenu({ source: "native", options });
      setSelectedAudio((current) =>
        current !== null && current < options.length
          ? current
          : audioOptionIndex(options, ticket.audioLanguage),
      );
    };
    element.addEventListener("loadedmetadata", read);
    return () => element.removeEventListener("loadedmetadata", read);
  }, [ticket]);

  // One writer for every path: the same endpoint and the same store the desktop uses.
  const report = useCallback(
    (keepalive = false): void => {
      const element = video.current;
      if (element === null) return;
      const position = Math.max(0, Math.round(ticket.startPositionMs + element.currentTime * 1000));
      const duration = totalMs ?? ticket.durationMs;
      void api.progress(titleId, ticket.episodeId, position, duration, keepalive).catch(() => {
        // A dropped report is one the next tick replaces; nothing on screen waits for it.
      });
    },
    [ticket, titleId, totalMs],
  );
  reportRef.current = report;

  useEffect(() => {
    const element = video.current;
    if (element === null) return;
    const onTime = (): void => {
      if (pending.current === null) {
        setAbsoluteMs(ticket.startPositionMs + element.currentTime * 1000);
      }
    };
    const onMetadata = (): void => {
      // An HLS event playlist reports the length encoded so far, not the title. The ticket's
      // probed duration wins; only a ticket without one lets the timeline grow with the encoder.
      const observed = totalDurationMs(ticket.durationMs, ticket.startPositionMs, element.duration);
      if (observed === null) return;
      setTotalMs((current) =>
        ticket.durationMs !== null ? observed : Math.max(current ?? 0, observed),
      );
    };
    const onPlay = (): void => {
      setPlaying(true);
      setFinished(false);
      // Replaying after a "Finished" is a new ending, and it owes its own progress report.
      endedReported.current = false;
    };
    const onPause = (): void => {
      setPlaying(false);
      reportRef.current();
    };
    const onEnded = (): void => {
      if (endedReported.current) return;
      endedReported.current = true;
      setPlaying(false);
      reportRef.current(true);
      const next = nextEpisodeRequest(ticket.nextEpisode);
      if (next === null) {
        setFinished(true);
        return;
      }
      if (!mounted.current || advancing.current) return;
      advancing.current = true;
      setStartingNext({
        seasonNumber: next.seasonNumber,
        episodeNumber: next.episodeNumber,
      });
      api
        .playback(titleId, next.positionMs, next.seasonNumber, next.episodeNumber)
        .then((minted) => {
          if (!mounted.current) {
            // The screen that would have played this is gone; do not leave its transcode running.
            void api.stopPlayback(minted.url).catch(() => {});
            return;
          }
          setStartingNext(null);
          setFinished(false);
          setTicket(minted);
        })
        .catch((caught: unknown) => {
          if (!mounted.current) return;
          setStartingNext(null);
          setMessage(
            caught instanceof Error ? caught.message : "The next episode could not be started.",
          );
        })
        .finally(() => {
          advancing.current = false;
        });
    };
    element.addEventListener("timeupdate", onTime);
    element.addEventListener("loadedmetadata", onMetadata);
    element.addEventListener("durationchange", onMetadata);
    element.addEventListener("play", onPlay);
    element.addEventListener("pause", onPause);
    element.addEventListener("ended", onEnded);
    return () => {
      element.removeEventListener("timeupdate", onTime);
      element.removeEventListener("loadedmetadata", onMetadata);
      element.removeEventListener("durationchange", onMetadata);
      element.removeEventListener("play", onPlay);
      element.removeEventListener("pause", onPause);
      element.removeEventListener("ended", onEnded);
    };
  }, [ticket, titleId]);

  useEffect(() => {
    const interval = window.setInterval(() => {
      const element = video.current;
      if (element !== null && !element.paused && !element.ended) reportRef.current();
    }, 10_000);
    return () => window.clearInterval(interval);
  }, []);

  useEffect(() => {
    const hidden = (): void => {
      if (document.visibilityState === "hidden") reportRef.current(true);
    };
    const leave = (): void => {
      reportRef.current(true);
      void api.stopPlayback(ticket.url).catch(() => {});
    };
    window.addEventListener("pagehide", leave);
    document.addEventListener("visibilitychange", hidden);
    return () => {
      window.removeEventListener("pagehide", leave);
      document.removeEventListener("visibilitychange", hidden);
      reportRef.current(true);
    };
  }, [ticket.url]);

  const restart = useCallback(
    async (target: number): Promise<void> => {
      pending.current = null;
      setPendingMs(null);
      setMessage("Starting at " + format(target) + "...");
      video.current?.pause();
      try {
        const next = await api.playback(
          titleId,
          Math.round(target),
          undefined,
          undefined,
          ticket.episodeId ?? undefined,
        );
        setTicket(next);
      } catch (caught) {
        setMessage(
          caught instanceof Error ? caught.message : "That position could not be started.",
        );
      }
    },
    [titleId, ticket.episodeId],
  );

  const seekTo = useCallback(
    (target: number): void => {
      const clamped = Math.max(0, Math.min(target, totalMs ?? target));
      pending.current = clamped;
      setPendingMs(clamped);
      if (timer.current !== null) window.clearTimeout(timer.current);
      timer.current = window.setTimeout(() => {
        void restart(clamped);
      }, 450);
    },
    [restart, totalMs],
  );

  const toggle = (): void => {
    const element = video.current;
    if (element === null) return;
    if (element.paused) {
      void element.play().catch(() => {});
    } else {
      element.pause();
    }
  };

  const selectAudio = (index: number): void => {
    const menu = audioMenu;
    if (menu === null) return;
    if (menu.source === "hls") {
      const instance = hls.current;
      if (instance !== null && index < instance.audioTracks.length) {
        instance.audioTrack = index;
      }
    } else {
      const list = nativeAudioTracks(video.current);
      if (list !== null) {
        for (let current = 0; current < list.length; current += 1) {
          const track = list[current];
          if (track !== undefined) track.enabled = current === index;
        }
      }
    }
    setSelectedAudio(index);
  };

  const close = (): void => {
    reportRef.current(true);
    void api.stopPlayback(ticket.url).catch(() => {});
    onClose();
  };

  const fullscreen = (): void => {
    const element = video.current;
    if (element === null) return;
    const ios = element as HTMLVideoElement & { webkitEnterFullscreen?: () => void };
    if (document.fullscreenElement !== null) {
      void document.exitFullscreen();
    } else if (typeof ios.webkitEnterFullscreen === "function") {
      ios.webkitEnterFullscreen();
    } else {
      void element.parentElement?.requestFullscreen().catch(() => {});
    }
  };

  const shown = pendingMs ?? absoluteMs;

  return (
    <main className="player">
      <button
        aria-label="Back"
        className="back-icon player-back"
        data-autofocus="true"
        data-focus="true"
        onClick={close}
        type="button"
      >
        <ChevronLeftIcon />
      </button>
      <video autoPlay playsInline ref={video}>
        {ticket.subtitles.map((track) => (
          <track
            default={track.default}
            key={track.id}
            kind="subtitles"
            label={subtitleLabel(track)}
            src={track.url}
            srcLang={track.language}
          />
        ))}
      </video>
      {battery !== null && battery.percent < 10 && !battery.charging ? (
        <p className="player-warning" role="alert">
          The computer running Peregrine is at {battery.percent}% battery. Playback stops when it
          runs out.
        </p>
      ) : null}
      {message === null ? null : <p className="player-message">{message}</p>}
      {startingNext === null && !finished ? null : (
        <p className="player-next" role="status">
          {startingNext === null
            ? "Finished"
            : "Starting S" +
              String(startingNext.seasonNumber) +
              " E" +
              String(startingNext.episodeNumber) +
              "..."}
        </p>
      )}
      <div className="player-controls">
        <button className="control" data-focus="true" onClick={toggle} type="button">
          {playing ? "Pause" : "Play"}
        </button>
        <button
          className="control"
          data-focus="true"
          onClick={() => seekTo(shown - 10_000)}
          type="button"
        >
          -10s
        </button>
        <button
          className="control"
          data-focus="true"
          onClick={() => seekTo(shown + 10_000)}
          type="button"
        >
          +10s
        </button>
        <button className="control" data-focus="true" onClick={fullscreen} type="button">
          Fullscreen
        </button>
        {ticket.subtitles.length === 0 ? null : (
          <label className="player-menu">
            <span className="player-menu-label">Subtitles</span>
            <select
              aria-label="Subtitles"
              className="player-select"
              data-focus="true"
              onChange={(event) => {
                const value = event.currentTarget.value;
                setSelectedSubtitle(value === "" ? null : Number(value));
              }}
              value={selectedSubtitle === null ? "" : String(selectedSubtitle)}
            >
              <option value="">Off</option>
              {ticket.subtitles.map((track, index) => (
                <option key={track.id} value={String(index)}>
                  {subtitleLabel(track)}
                </option>
              ))}
            </select>
          </label>
        )}
        {ticket.audioLanguage === null && audioMenu === null ? null : (
          <span className="player-audio-group">
            <span className="player-audio">
              {ticket.audioLanguage === null ? "Audio" : "Audio: " + ticket.audioLanguage}
            </span>
            {audioMenu === null ? null : (
              <select
                aria-label="Audio track"
                className="player-select"
                data-focus="true"
                onChange={(event) => selectAudio(Number(event.currentTarget.value))}
                value={selectedAudio === null ? "" : String(selectedAudio)}
              >
                {audioMenu.options.map((option, index) => (
                  <option key={option.id} value={String(index)}>
                    {option.label}
                  </option>
                ))}
              </select>
            )}
          </span>
        )}
        <span className="time">
          {format(shown)} / {totalMs === null ? "--:--" : format(totalMs)}
        </span>
        <input
          aria-label="Seek"
          className="seek"
          data-focus="true"
          max={totalMs ?? 0}
          min={0}
          onChange={(event) => seekTo(Number(event.target.value))}
          step={1000}
          type="range"
          value={Math.min(shown, totalMs ?? 0)}
        />
        <span className="player-title">{titleName}</span>
      </div>
    </main>
  );
}

function format(milliseconds: number): string {
  const total = Math.max(0, Math.round(milliseconds / 1000));
  const hours = Math.floor(total / 3600);
  const minutes = Math.floor((total % 3600) / 60);
  const seconds = total % 60;
  const pad = (value: number): string => String(value).padStart(2, "0");
  return hours > 0
    ? String(hours) + ":" + pad(minutes) + ":" + pad(seconds)
    : String(minutes) + ":" + pad(seconds);
}
