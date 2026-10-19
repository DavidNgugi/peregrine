/**
 * The player: direct bytes when the browser demuxes the file, HLS when the host transcodes it,
 * and one progress writer either way.
 *
 * The timestamp the person sees is absolute in the title, while the media element's clock starts at
 * whatever offset the transcode was started at. Seeking is therefore a request for a new session at
 * the target offset, not a call to `currentTime`: a transcode cannot seek backwards, and a seek
 * beyond what the encoder has produced would stall. The debounce turns a held arrow key into one
 * restart.
 */

import type Hls from "hls.js";
import { useCallback, useEffect, useRef, useState, type ReactElement } from "react";

import { api, type PlaybackTicket } from "./api.js";

export interface PlayerProps {
  readonly titleId: number;
  readonly titleName: string;
  readonly initial: PlaybackTicket;
  readonly onClose: () => void;
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
    if (ticket.kind === "hls") {
      void import("hls.js")
        .then(({ default: HlsPlayer }) => {
          if (cancelled) return;
          if (HlsPlayer.isSupported()) {
            const instance = new HlsPlayer({ enableWorker: true });
            hls.current = instance;
            instance.on(HlsPlayer.Events.ERROR, (_event, data) => {
              if (data.fatal) setMessage("The transcode stopped. Choose a position to try again.");
            });
            instance.loadSource(ticket.url);
            instance.attachMedia(element);
            instance.on(HlsPlayer.Events.MANIFEST_PARSED, () => {
              void element.play().catch(() => {
                setPlaying(false);
              });
            });
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
      if (Number.isFinite(element.duration) && element.duration > 0) {
        setTotalMs(ticket.startPositionMs + element.duration * 1000);
      }
    };
    const onPlay = (): void => setPlaying(true);
    const onPause = (): void => {
      setPlaying(false);
      reportRef.current();
    };
    const onEnded = (): void => {
      setPlaying(false);
      reportRef.current(true);
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
  }, [ticket]);

  useEffect(() => {
    const interval = window.setInterval(() => {
      const element = video.current;
      if (element !== null && !element.paused && !element.ended) reportRef.current();
    }, 10_000);
    return () => window.clearInterval(interval);
  }, []);

  useEffect(() => {
    const flush = (): void => {
      if (document.visibilityState === "hidden") reportRef.current(true);
    };
    window.addEventListener("pagehide", flush);
    document.addEventListener("visibilitychange", flush);
    return () => {
      window.removeEventListener("pagehide", flush);
      document.removeEventListener("visibilitychange", flush);
      reportRef.current(true);
    };
  }, []);

  const restart = useCallback(
    async (target: number): Promise<void> => {
      pending.current = null;
      setPendingMs(null);
      setMessage("Starting at " + format(target) + "...");
      video.current?.pause();
      try {
        const next = await api.playback(titleId, Math.round(target));
        setTicket(next);
      } catch (caught) {
        setMessage(
          caught instanceof Error ? caught.message : "That position could not be started.",
        );
      }
    },
    [titleId],
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

  const close = (): void => {
    reportRef.current(true);
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
      <video autoPlay playsInline ref={video} />
      {message === null ? null : <p className="player-message">{message}</p>}
      <div className="player-controls">
        <button
          className="back"
          data-autofocus="true"
          data-focus="true"
          onClick={close}
          type="button"
        >
          Back
        </button>
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
