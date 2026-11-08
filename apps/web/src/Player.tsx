/**
 * The player: direct bytes when the browser demuxes the file, HLS when the host transcodes it,
 * and one progress writer either way.
 *
 * The timestamp the person sees is absolute in the title, while the media element's clock starts at
 * whatever offset the transcode was started at. A seek is therefore resolved first against what the
 * current session already holds — the whole file for a direct play, the encoded window for a
 * transcode — and only a target outside it becomes a new session at that offset. A scrub is held
 * as a pending position and committed once the keys go quiet, so a held arrow is one seek.
 *
 * **The remote has a grammar while the controls are hidden** (see `player-controls.ts`): OK pauses
 * or takes the on-screen skip, left and right scrub, up and down reveal the controls. Once they are
 * showing, spatial focus owns the arrows again, except on the timeline, where left and right
 * scrub. Back closes the track panel or abandons a scrub before it leaves the player.
 *
 * **The sidecars are served by the listener, never built here.** Every subtitle URL in the ticket
 * already answers WebVTT and, for an HLS session, already carries the transcode's offset. A track
 * is chosen by its id rather than its position in `textTracks`, because hls.js may add caption
 * tracks of its own ahead of or between the sidecars.
 *
 * **The shared post-play coordinator owns what follows an ending.** A credits marker or the
 * `ended` event opens it; a countdown for the next episode starts only when the ticket offered
 * one, and one of its commands starts exactly one mint. A pause, a seek, Cancel, or the back
 * control never starts a follow-up.
 */

import type Hls from "hls.js";
import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type MouseEvent as ReactMouseEvent,
  type ReactElement,
  type RefObject,
} from "react";

import {
  activeMarker,
  advance,
  type NextUp,
  type PostPlayCommand,
  type PostPlayEvent,
  type PostPlayState,
} from "@peregrine/player-ui";

import { api, type Marker, type PlaybackTicket } from "./api.js";
import {
  CheckIcon,
  ChevronLeftIcon,
  ForwardIcon,
  FullscreenIcon,
  NextIcon,
  PauseIcon,
  PlayIcon,
  RewindIcon,
  SubtitlesIcon,
} from "./icons.js";
import {
  CONTROLS_IDLE_MS,
  SCRUB_COMMIT_MS,
  inSessionSeconds,
  nextScrubRun,
  playerKey,
  progressPercent,
  scrubStepMs,
  scrubTarget,
  type ScrubRun,
} from "./player-controls.js";
import {
  absolutePositionMs,
  hiddenPostPlay,
  nextUpLabel,
  skipTarget,
  toNextUp,
  toPlayerMarkers,
} from "./player-postplay.js";
import {
  audioOptionIndex,
  initialSubtitleIndex,
  nextEpisodeRequest,
  subtitleLabel,
  totalDurationMs,
} from "./player-tracks.js";
import { isTizen, transportAction, useBackInterceptor } from "./tizen.js";

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

/** The control a reveal should leave focused. */
type FocusTarget = "timeline" | "play" | "tracks";

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

/** The `<track>` id a sidecar is rendered with, and therefore the `TextTrack.id` it is found by. */
function subtitleElementId(trackId: number): string {
  return "subtitle-" + String(trackId);
}

/** The end of what the element can seek to now, in its own seconds. */
function seekableEnd(element: HTMLVideoElement): number | null {
  const ranges = element.seekable;
  if (ranges.length === 0) return null;
  return ranges.end(ranges.length - 1);
}

const TIZEN = typeof navigator !== "undefined" && isTizen(navigator.userAgent);

export function Player({ titleId, titleName, initial, onClose }: PlayerProps): ReactElement {
  const video = useRef<HTMLVideoElement>(null);
  const hls = useRef<Hls | null>(null);
  const root = useRef<HTMLElement>(null);
  const timeline = useRef<HTMLDivElement>(null);
  const playButton = useRef<HTMLButtonElement>(null);
  const tracksButton = useRef<HTMLButtonElement>(null);
  const tracksPanel = useRef<HTMLElement>(null);
  const [ticket, setTicket] = useState<PlaybackTicket>(initial);
  const [playing, setPlaying] = useState(false);
  const [waiting, setWaiting] = useState(true);
  const [absoluteMs, setAbsoluteMs] = useState(initial.startPositionMs);
  const [totalMs, setTotalMs] = useState<number | null>(initial.durationMs);
  const [pendingMs, setPendingMs] = useState<number | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  const [controls, setControls] = useState(true);
  const [panelOpen, setPanelOpen] = useState(false);
  const [activity, setActivity] = useState(0);
  const [focusRequest, setFocusRequest] = useState<{
    readonly target: FocusTarget;
    readonly serial: number;
  } | null>(null);
  const pending = useRef<number | null>(null);
  const timer = useRef<number | null>(null);
  const scrubRun = useRef<ScrubRun | null>(null);
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
  const [markers, setMarkers] = useState<readonly Marker[]>(initial.markers);
  // The ticket whose state the marker and post-play values below belong to. A ticket change and
  // its reset are separate renders, and without this a stale credits position could open the panel
  // on the episode that just started.
  const [sessionUrl, setSessionUrl] = useState(initial.url);
  const [postPlay, setPostPlay] = useState<PostPlayState>(() =>
    hiddenPostPlay(toNextUp(initial.nextEpisode, titleId)),
  );

  // A mint can outlive the screen that asked for it; these refs are how a late answer is dropped.
  const mounted = useRef(true);
  const advancing = useRef(false);
  const endedReported = useRef(false);
  const finalizedReported = useRef(false);
  const latestTicket = useRef(ticket);
  latestTicket.current = ticket;

  // The coordinator is the only owner of the post-play phase; these refs let an event listener and
  // the countdown interval read the latest value without re-subscribing on every tick.
  const postPlayRef = useRef<PostPlayState>(postPlay);
  const postPlayDispatch = useRef<(event: PostPlayEvent) => void>(() => {});
  const postPlayCommand = useRef<(next: PostPlayCommand | null) => void>(() => {});
  const openedFor = useRef<string | null>(null);
  const backButton = useRef<HTMLButtonElement>(null);
  const panel = useRef<HTMLElement>(null);

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

  // Attach the chosen delivery. A ticket change is a new session, so the old one is destroyed
  // before the new URL is loaded.
  useEffect(() => {
    const element = video.current;
    if (element === null) return;
    let cancelled = false;
    hls.current?.destroy();
    hls.current = null;
    setMessage(null);
    setWaiting(true);
    setTotalMs(ticket.durationMs);
    setAbsoluteMs(ticket.startPositionMs);
    pending.current = null;
    setPendingMs(null);
    setAudioMenu(null);
    setSelectedAudio(null);
    setSessionUrl(ticket.url);
    setMarkers(ticket.markers);
    const fresh = hiddenPostPlay(toNextUp(ticket.nextEpisode, titleId));
    postPlayRef.current = fresh;
    setPostPlay(fresh);
    openedFor.current = null;
    endedReported.current = false;
    finalizedReported.current = false;
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
  }, [ticket, titleId]);

  // The ticket's markers are the first read; the mint starts a background refresh, so two later
  // reads pick up a source that answered after the first frame. The cleanup is what stops those
  // reads from landing on a screen that has moved on.
  useEffect(() => {
    let cancelled = false;
    const read = (): void => {
      void api
        .markers(titleId, ticket.episodeId, ticket.durationMs)
        .then((found) => {
          if (!cancelled) setMarkers(found);
        })
        .catch(() => {
          // A refresh that fails leaves the ticket's first read in place.
        });
    };
    const first = window.setTimeout(read, 2000);
    const second = window.setTimeout(read, 6000);
    return () => {
      cancelled = true;
      window.clearTimeout(first);
      window.clearTimeout(second);
    };
  }, [ticket, titleId]);

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
    const chosen = selectedSubtitle === null ? undefined : ticket.subtitles[selectedSubtitle];
    const wanted = chosen === undefined ? null : subtitleElementId(chosen.id);
    const apply = (): void => {
      for (let index = 0; index < element.textTracks.length; index += 1) {
        const entry = element.textTracks[index];
        if (entry === undefined) continue;
        entry.mode = wanted !== null && entry.id === wanted ? "showing" : "disabled";
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
      const position = Math.max(
        0,
        Math.round(absolutePositionMs(ticket.kind, ticket.startPositionMs, element.currentTime)),
      );
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
        setAbsoluteMs(absolutePositionMs(ticket.kind, ticket.startPositionMs, element.currentTime));
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
      // Playing again after an ending owes its own progress report, and the post-play panel stays
      // under the coordinator's control rather than being cleared here.
      endedReported.current = false;
    };
    const onPause = (): void => {
      setPlaying(false);
      // A paused picture keeps its controls; the idle timer only hides them while playing.
      setControls(true);
      reportRef.current();
    };
    const onWaiting = (): void => setWaiting(true);
    const onFlowing = (): void => setWaiting(false);
    const onEnded = (): void => {
      if (endedReported.current) return;
      endedReported.current = true;
      finalizedReported.current = true;
      setPlaying(false);
      const elementDuration = Number.isFinite(element.duration)
        ? Math.round(element.duration * 1000)
        : ticket.durationMs;
      const position = Math.max(
        0,
        Math.round(absolutePositionMs(ticket.kind, ticket.startPositionMs, element.currentTime)),
      );
      void api
        .progress(titleId, ticket.episodeId, position, totalMs ?? elementDuration, true, true)
        .catch(() => {});
      // The coordinator decides between a countdown and the ended panel; a film never reaches a
      // black screen, and a cancelled session cannot start anything from here.
      postPlayDispatch.current({ type: "ended" });
    };
    element.addEventListener("timeupdate", onTime);
    element.addEventListener("loadedmetadata", onMetadata);
    element.addEventListener("durationchange", onMetadata);
    element.addEventListener("play", onPlay);
    element.addEventListener("pause", onPause);
    element.addEventListener("waiting", onWaiting);
    element.addEventListener("seeking", onWaiting);
    element.addEventListener("playing", onFlowing);
    element.addEventListener("seeked", onFlowing);
    element.addEventListener("canplay", onFlowing);
    element.addEventListener("ended", onEnded);
    return () => {
      element.removeEventListener("timeupdate", onTime);
      element.removeEventListener("loadedmetadata", onMetadata);
      element.removeEventListener("durationchange", onMetadata);
      element.removeEventListener("play", onPlay);
      element.removeEventListener("pause", onPause);
      element.removeEventListener("waiting", onWaiting);
      element.removeEventListener("seeking", onWaiting);
      element.removeEventListener("playing", onFlowing);
      element.removeEventListener("seeked", onFlowing);
      element.removeEventListener("canplay", onFlowing);
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
        if (!mounted.current) {
          // A seek, a skip, or a replay can outlive the screen that asked for it.
          void api.stopPlayback(next.url).catch(() => {});
          return;
        }
        setTicket(next);
      } catch (caught) {
        if (!mounted.current) return;
        setMessage(
          caught instanceof Error ? caught.message : "That position could not be started.",
        );
      }
    },
    [titleId, ticket.episodeId],
  );

  const clearScrubTimer = (): void => {
    if (timer.current !== null) {
      window.clearTimeout(timer.current);
      timer.current = null;
    }
  };

  /** Moves playback to `targetMs` now: inside the session when it holds the target, else a new one. */
  const commitSeek = useCallback(
    (targetMs: number): void => {
      clearScrubTimer();
      scrubRun.current = null;
      pending.current = null;
      setPendingMs(null);
      const element = video.current;
      const seconds =
        element === null
          ? null
          : inSessionSeconds(ticket.kind, ticket.startPositionMs, targetMs, seekableEnd(element));
      if (element !== null && seconds !== null) {
        setAbsoluteMs(targetMs);
        element.currentTime = seconds;
        return;
      }
      void restart(targetMs);
    },
    [restart, ticket.kind, ticket.startPositionMs],
  );

  /** Holds `target` as the scrub position and commits it once the keys go quiet. */
  const scrubTo = useCallback(
    (target: number): void => {
      pending.current = target;
      setPendingMs(target);
      clearScrubTimer();
      timer.current = window.setTimeout(() => {
        timer.current = null;
        commitSeek(target);
      }, SCRUB_COMMIT_MS);
    },
    [commitSeek],
  );

  const shown = pendingMs ?? absoluteMs;
  const shownRef = useRef(shown);
  shownRef.current = shown;

  /** One press of a scrub key: the step grows while the key is held. */
  const scrub = useCallback(
    (direction: -1 | 1, base: number): void => {
      const run = nextScrubRun(scrubRun.current, direction, Date.now());
      scrubRun.current = run;
      const from = pending.current ?? shownRef.current;
      scrubTo(scrubTarget(from, direction * scrubStepMs(run.count, base), totalMs));
    },
    [scrubTo, totalMs],
  );

  const cancelScrub = (): void => {
    clearScrubTimer();
    scrubRun.current = null;
    pending.current = null;
    setPendingMs(null);
  };

  const toggle = (): void => {
    const element = video.current;
    if (element === null) return;
    if (element.paused) {
      void element.play().catch(() => {});
    } else {
      element.pause();
    }
  };

  const reveal = (focus: FocusTarget | null): void => {
    setControls(true);
    setActivity((value) => value + 1);
    if (focus !== null) {
      setFocusRequest((current) => ({ target: focus, serial: (current?.serial ?? 0) + 1 }));
    }
  };

  // Focus is placed after the render that makes the target visible; a hidden control cannot take it.
  useEffect(() => {
    if (focusRequest === null || !controls) return;
    const target =
      focusRequest.target === "timeline"
        ? timeline.current
        : focusRequest.target === "tracks"
          ? tracksButton.current
          : playButton.current;
    target?.focus({ preventScroll: true });
  }, [focusRequest, controls]);

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
    // Cancel is the coordinator's own answer to leaving: it clears the offer before the unmount
    // cleanup clears the interval, so no tick can outlive the screen.
    postPlayDispatch.current({ type: "cancel" });
    reportRef.current(true);
    void api.stopPlayback(ticket.url).catch(() => {});
    onClose();
  };

  const canFullscreen =
    !TIZEN && typeof document !== "undefined" && document.fullscreenEnabled !== false;

  const fullscreen = (): void => {
    const element = video.current;
    if (element === null) return;
    const ios = element as HTMLVideoElement & { webkitEnterFullscreen?: () => void };
    if (document.fullscreenElement !== null) {
      void document.exitFullscreen();
    } else if (typeof root.current?.requestFullscreen === "function") {
      // The whole player goes full screen, not the element, so the controls and panels come along.
      void root.current.requestFullscreen().catch(() => {});
    } else if (typeof ios.webkitEnterFullscreen === "function") {
      ios.webkitEnterFullscreen();
    }
  };

  const openPanel = (): void => {
    setPanelOpen(true);
    setControls(true);
  };

  const closePanel = (): void => {
    setPanelOpen(false);
    setFocusRequest((current) => ({ target: "tracks", serial: (current?.serial ?? 0) + 1 }));
  };

  // The panel takes focus on its selected entry, so OK on open never changes a choice by itself.
  useEffect(() => {
    if (!panelOpen) return;
    const target =
      tracksPanel.current?.querySelector<HTMLElement>('[aria-checked="true"]') ??
      tracksPanel.current?.querySelector<HTMLElement>("[data-focus]");
    target?.focus({ preventScroll: true });
  }, [panelOpen]);

  useBackInterceptor(() => {
    if (panelOpen) {
      closePanel();
      return true;
    }
    if (pending.current !== null) {
      cancelScrub();
      return true;
    }
    return false;
  });

  const playerMarkers = useMemo(() => toPlayerMarkers(markers), [markers]);
  const markerPosition = shown;
  const skip = skipTarget(playerMarkers, markerPosition);
  const liveMarker = activeMarker(playerMarkers, markerPosition);
  const creditsKey =
    sessionUrl === ticket.url && liveMarker !== null && liveMarker.kind === "credits"
      ? ticket.url + ":" + String(liveMarker.startMs)
      : null;

  const startNext = useCallback((): void => {
    const next = nextEpisodeRequest(ticket.nextEpisode);
    if (next === null || !mounted.current || advancing.current) return;
    advancing.current = true;
    setStartingNext({ seasonNumber: next.seasonNumber, episodeNumber: next.episodeNumber });
    api
      .playback(titleId, next.positionMs, next.seasonNumber, next.episodeNumber)
      .then((minted) => {
        if (!mounted.current) {
          // The screen that would have played this is gone; do not leave its transcode running.
          void api.stopPlayback(minted.url).catch(() => {});
          return;
        }
        setStartingNext(null);
        const fresh = hiddenPostPlay(toNextUp(minted.nextEpisode, titleId));
        postPlayRef.current = fresh;
        setPostPlay(fresh);
        openedFor.current = null;
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
  }, [ticket.nextEpisode, titleId]);

  // A replay inside the session reuses the ticket, so the ending's state is reset here rather than
  // by the ticket effect a restart would have run.
  const replay = useCallback((): void => {
    const fresh = hiddenPostPlay(toNextUp(ticket.nextEpisode, titleId));
    postPlayRef.current = fresh;
    setPostPlay(fresh);
    openedFor.current = null;
    endedReported.current = false;
    commitSeek(0);
    void video.current?.play().catch(() => {});
  }, [commitSeek, ticket.nextEpisode, titleId]);

  const runCommand = useCallback(
    (command: PostPlayCommand | null): void => {
      switch (command) {
        case null:
          return;
        case "start-next":
          startNext();
          return;
        case "replay":
          replay();
          return;
        case "credits":
          // The overlay has no Watch credits action; the credits command is unreachable here.
          return;
        case "close":
          onClose();
          return;
      }
    },
    [onClose, replay, startNext],
  );

  // The coordinator owns the transition; this applies its state and executes its one command.
  const dispatchPostPlay = useCallback((event: PostPlayEvent): void => {
    const result = advance(postPlayRef.current, event);
    postPlayRef.current = result.state;
    setPostPlay(result.state);
    if (result.command !== null) postPlayCommand.current(result.command);
  }, []);

  postPlayDispatch.current = dispatchPostPlay;
  postPlayCommand.current = runCommand;

  // The countdown has no timer of its own; the shell supplies the elapsed time and the coordinator
  // decides when the single start-next is due. A state change or unmount clears the interval.
  useEffect(() => {
    if (postPlay.phase !== "countdown") return;
    const timer = window.setInterval(() => {
      postPlayDispatch.current({ type: "tick", elapsedMs: 1000 });
    }, 1000);
    return () => window.clearInterval(timer);
  }, [postPlay.phase]);

  // Focus is moved into the panel when it opens, but never away from Back: a remote user who
  // navigated there keeps the exit they chose.
  useEffect(() => {
    if (postPlay.phase === "hidden") return;
    const active = document.activeElement;
    if (active === backButton.current) return;
    // A scrub in progress keeps the timeline; a marker reached under the thumb must not pull the
    // focus out of it.
    if (active === timeline.current && pending.current !== null) return;
    const target = panel.current;
    if (target === null) return;
    if (active !== null && target.contains(active)) return;
    target.querySelector<HTMLElement>("[data-autofocus]")?.focus();
  }, [postPlay.phase]);

  // Reaching a credits marker opens the post-play surface once. The guard outlives a cancel, so a
  // later position report inside the same credits window cannot resurrect a declined follow-up.
  useEffect(() => {
    if (creditsKey === null || postPlay.phase !== "hidden") return;
    if (openedFor.current === creditsKey) return;
    openedFor.current = creditsKey;
    if (!finalizedReported.current) {
      finalizedReported.current = true;
      void api
        .progress(titleId, ticket.episodeId, shown, totalMs ?? ticket.durationMs, true, true)
        .catch(() => {});
    }
    postPlayDispatch.current({ type: "credits" });
  }, [creditsKey, postPlay.phase, shown, ticket, titleId, totalMs]);

  // The controls leave once the picture has had a quiet stretch; a pause, a scrub, an open panel,
  // or the post-play surface keeps them, because each is something the person is in the middle of.
  const postPlayOpen = postPlay.phase !== "hidden";
  useEffect(() => {
    if (!controls || !playing || panelOpen || pendingMs !== null || postPlayOpen) return;
    const hide = window.setTimeout(() => {
      const active = document.activeElement;
      if (active instanceof HTMLElement && root.current?.contains(active)) active.blur();
      setControls(false);
    }, CONTROLS_IDLE_MS);
    return () => window.clearTimeout(hide);
  }, [controls, playing, panelOpen, pendingMs, postPlayOpen, activity]);

  // The remote, read in the capture phase so a key the player owns never reaches spatial focus.
  // The ref keeps one listener across the four updates a second that `shown` changes.
  const keys = useRef({
    close,
    commitSeek,
    scrub,
    toggle,
    reveal,
    skip,
    controls,
    panelOpen,
    postPlayOpen,
  });
  keys.current = {
    close,
    commitSeek,
    scrub,
    toggle,
    reveal,
    skip,
    controls,
    panelOpen,
    postPlayOpen,
  };
  useEffect(() => {
    const onKey = (event: KeyboardEvent): void => {
      const current = keys.current;
      const element = video.current;
      const action = transportAction(event.keyCode, event.key);
      if (action !== null) {
        event.preventDefault();
        event.stopPropagation();
        switch (action) {
          case "play":
            if (element !== null) void element.play().catch(() => {});
            current.reveal(null);
            break;
          case "pause":
            element?.pause();
            break;
          case "toggle":
            current.toggle();
            current.reveal(null);
            break;
          case "stop":
            current.close();
            break;
          case "forward":
            current.scrub(1, 30_000);
            current.reveal("timeline");
            break;
          case "rewind":
            current.scrub(-1, 30_000);
            current.reveal("timeline");
            break;
        }
        return;
      }
      const key = playerKey(event.key, event.keyCode);
      if (key === null) return;
      setActivity((value) => value + 1);
      // An open panel and the post-play surface are ordinary focus screens.
      if (current.panelOpen || current.postPlayOpen) return;
      const active = document.activeElement;
      const onControl =
        current.controls &&
        active instanceof HTMLElement &&
        active.matches("[data-focus]") &&
        root.current?.contains(active) === true;
      if (onControl && active === timeline.current) {
        if (key === "left" || key === "right") {
          event.preventDefault();
          event.stopPropagation();
          current.scrub(key === "left" ? -1 : 1, 10_000);
          return;
        }
        if (key === "ok") {
          event.preventDefault();
          event.stopPropagation();
          if (pending.current !== null) current.commitSeek(pending.current);
          else current.toggle();
          return;
        }
      }
      if (onControl) return;
      // Nothing of the player's is focused: the picture itself answers the D-pad.
      event.preventDefault();
      event.stopPropagation();
      switch (key) {
        case "ok":
          if (current.skip !== null) current.commitSeek(current.skip.targetMs);
          else current.toggle();
          current.reveal("play");
          break;
        case "left":
        case "right":
          current.scrub(key === "left" ? -1 : 1, 10_000);
          current.reveal("timeline");
          break;
        case "up":
        case "down":
          current.reveal(key === "up" ? "timeline" : "play");
          break;
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, []);

  // A pointer reports dozens of moves a second; one reveal per half second keeps the idle timer
  // honest without a render per move.
  const lastPointer = useRef(0);
  const onPointerMove = (): void => {
    const now = Date.now();
    if (controls && now - lastPointer.current < 500) return;
    lastPointer.current = now;
    reveal(null);
  };

  const onSurfaceClick = (): void => {
    if (!controls) {
      reveal(null);
      return;
    }
    toggle();
  };

  const onTimelineClick = (event: ReactMouseEvent<HTMLDivElement>): void => {
    if (totalMs === null) return;
    const box = event.currentTarget.getBoundingClientRect();
    if (box.width <= 0) return;
    const ratio = Math.min(1, Math.max(0, (event.clientX - box.left) / box.width));
    commitSeek(Math.round(ratio * totalMs));
  };

  const focusable = controls ? "true" : undefined;
  const percent = progressPercent(shown, totalMs);
  const played = progressPercent(absoluteMs, totalMs);
  const remaining = totalMs === null ? null : Math.max(0, totalMs - shown);
  const subtitleChoice = selectedSubtitle === null ? undefined : ticket.subtitles[selectedSubtitle];

  return (
    <main
      className={
        "player" +
        (controls ? " player-active" : " player-idle") +
        (panelOpen ? " player-panel-open" : "")
      }
      onPointerMove={onPointerMove}
      ref={root}
    >
      <video autoPlay onClick={onSurfaceClick} playsInline ref={video}>
        {ticket.subtitles.map((track) => (
          <track
            default={track.default}
            id={subtitleElementId(track.id)}
            key={track.id}
            kind="subtitles"
            label={subtitleLabel(track)}
            src={track.url}
            srcLang={track.language}
          />
        ))}
      </video>
      {waiting && message === null ? (
        <div aria-label="Loading" className="player-spinner" role="status" />
      ) : null}
      {battery !== null && battery.percent < 10 && !battery.charging ? (
        <p className="player-warning" role="alert">
          The computer running Peregrine is at {battery.percent}% battery. Playback stops when it
          runs out.
        </p>
      ) : null}
      {message === null ? null : <p className="player-message">{message}</p>}

      <div aria-hidden={!controls} className="player-osd">
        <div className="player-top">
          <button
            aria-label="Back"
            className="player-icon-button"
            data-focus={focusable}
            onClick={close}
            ref={backButton}
            tabIndex={controls ? 0 : -1}
            type="button"
          >
            <ChevronLeftIcon />
          </button>
          <div className="player-heading">
            <p className="player-title">{titleName}</p>
            {subtitleChoice === undefined ? null : (
              <p className="player-subtitle-state">{subtitleLabel(subtitleChoice)}</p>
            )}
          </div>
        </div>
        <div className="player-bottom">
          <div
            aria-label="Seek"
            aria-valuemax={totalMs ?? 0}
            aria-valuemin={0}
            aria-valuenow={Math.round(shown)}
            aria-valuetext={format(shown)}
            className={"timeline" + (pendingMs === null ? "" : " timeline-scrubbing")}
            data-focus={focusable}
            onClick={onTimelineClick}
            ref={timeline}
            role="slider"
            tabIndex={controls ? 0 : -1}
          >
            <div className="timeline-track">
              <div
                className="timeline-played"
                style={{ transform: "scaleX(" + String(played / 100) + ")" }}
              />
              {playerMarkers.map((marker) =>
                totalMs === null || marker.kind === "credits" ? null : (
                  <div
                    className="timeline-marker"
                    key={marker.id}
                    style={{
                      transform:
                        "translateX(" + String(progressPercent(marker.startMs, totalMs)) + "%)",
                    }}
                  />
                ),
              )}
            </div>
            <div
              className="timeline-head"
              style={{ transform: "translateX(" + String(percent) + "%)" }}
            >
              <span className="timeline-thumb" />
              {pendingMs === null ? null : (
                <span className="timeline-bubble">{format(pendingMs)}</span>
              )}
            </div>
          </div>
          <div className="player-row">
            <span className="player-time">{format(shown)}</span>
            <div className="player-buttons">
              <button
                aria-label="Back 10 seconds"
                className="player-icon-button"
                data-focus={focusable}
                onClick={() => commitSeek(Math.max(0, shown - 10_000))}
                tabIndex={controls ? 0 : -1}
                type="button"
              >
                <RewindIcon />
              </button>
              <button
                aria-label={playing ? "Pause" : "Play"}
                className="player-icon-button player-play"
                data-focus={focusable}
                onClick={toggle}
                ref={playButton}
                tabIndex={controls ? 0 : -1}
                type="button"
              >
                {playing ? <PauseIcon /> : <PlayIcon />}
              </button>
              <button
                aria-label="Forward 10 seconds"
                className="player-icon-button"
                data-focus={focusable}
                onClick={() => commitSeek(scrubTarget(shown, 10_000, totalMs))}
                tabIndex={controls ? 0 : -1}
                type="button"
              >
                <ForwardIcon />
              </button>
              <button
                aria-expanded={panelOpen}
                aria-label="Audio and subtitles"
                className="player-icon-button"
                data-focus={focusable}
                onClick={openPanel}
                ref={tracksButton}
                tabIndex={controls ? 0 : -1}
                type="button"
              >
                <SubtitlesIcon />
              </button>
              {ticket.nextEpisode === null ? null : (
                <button
                  aria-label="Next episode"
                  className="player-icon-button"
                  data-focus={focusable}
                  onClick={startNext}
                  tabIndex={controls ? 0 : -1}
                  type="button"
                >
                  <NextIcon />
                </button>
              )}
              {canFullscreen ? (
                <button
                  aria-label="Full screen"
                  className="player-icon-button"
                  data-focus={focusable}
                  onClick={fullscreen}
                  tabIndex={controls ? 0 : -1}
                  type="button"
                >
                  <FullscreenIcon />
                </button>
              ) : null}
            </div>
            <span className="player-time player-remaining">
              {remaining === null ? "--:--" : "-" + format(remaining)}
            </span>
          </div>
        </div>
      </div>

      {skip === null || postPlayOpen ? null : (
        <button
          className="player-skip"
          data-focus="true"
          onClick={() => commitSeek(skip.targetMs)}
          type="button"
        >
          {skip.label}
        </button>
      )}
      {startingNext === null ? null : (
        <p className="player-next" role="status">
          {"Starting S" +
            String(startingNext.seasonNumber) +
            " E" +
            String(startingNext.episodeNumber) +
            "..."}
        </p>
      )}
      {panelOpen ? (
        <TracksPanel
          audio={audioMenu}
          audioLanguage={ticket.audioLanguage}
          onAudio={selectAudio}
          onClose={closePanel}
          onSubtitle={setSelectedSubtitle}
          panelRef={tracksPanel}
          selectedAudio={selectedAudio}
          selectedSubtitle={selectedSubtitle}
          subtitles={ticket.subtitles}
        />
      ) : null}
      <PostPlayPanel
        onCancel={() => postPlayDispatch.current({ type: "cancel" })}
        onPlayNow={() => postPlayDispatch.current({ type: "start-next" })}
        onReplay={() => postPlayDispatch.current({ type: "replay" })}
        panelRef={panel}
        state={postPlay}
      />
    </main>
  );
}

/**
 * The audio and subtitle sheet: two lists of plain buttons rather than native selects, because a
 * television's select popup is a system dialog the D-pad drives badly. The scope keeps spatial
 * focus inside the sheet until Back or Close returns it to the controls.
 */
export function TracksPanel({
  subtitles,
  selectedSubtitle,
  onSubtitle,
  audio,
  audioLanguage,
  selectedAudio,
  onAudio,
  onClose,
  panelRef,
}: {
  readonly subtitles: PlaybackTicket["subtitles"];
  readonly selectedSubtitle: number | null;
  readonly onSubtitle: (index: number | null) => void;
  readonly audio: AudioMenu | null;
  readonly audioLanguage: string | null;
  readonly selectedAudio: number | null;
  readonly onAudio: (index: number) => void;
  readonly onClose: () => void;
  readonly panelRef?: RefObject<HTMLElement | null> | undefined;
}): ReactElement {
  return (
    <section
      aria-label="Audio and subtitles"
      aria-modal="true"
      className="player-sheet"
      data-focus-scope="true"
      ref={panelRef}
      role="dialog"
    >
      <div className="player-sheet-columns">
        <div className="player-sheet-column">
          <h2>Audio</h2>
          {audio === null ? (
            <p className="player-sheet-note">
              {audioLanguage === null ? "One audio track" : "Audio: " + audioLanguage}
            </p>
          ) : (
            <ul role="radiogroup">
              {audio.options.map((option, index) => (
                <li key={option.id}>
                  <TrackOption
                    checked={selectedAudio === index}
                    label={option.label}
                    onSelect={() => onAudio(index)}
                  />
                </li>
              ))}
            </ul>
          )}
        </div>
        <div className="player-sheet-column">
          <h2>Subtitles</h2>
          <ul role="radiogroup">
            <li>
              <TrackOption
                checked={selectedSubtitle === null}
                label="Off"
                onSelect={() => onSubtitle(null)}
              />
            </li>
            {subtitles.map((track, index) => (
              <li key={track.id}>
                <TrackOption
                  checked={selectedSubtitle === index}
                  label={subtitleLabel(track)}
                  onSelect={() => onSubtitle(index)}
                />
              </li>
            ))}
          </ul>
          {subtitles.length === 0 ? (
            <p className="player-sheet-note">This file has no text subtitles.</p>
          ) : null}
        </div>
      </div>
      <button className="player-sheet-close" data-focus="true" onClick={onClose} type="button">
        Close
      </button>
    </section>
  );
}

function TrackOption({
  checked,
  label,
  onSelect,
}: {
  readonly checked: boolean;
  readonly label: string;
  readonly onSelect: () => void;
}): ReactElement {
  return (
    <button
      aria-checked={checked}
      className="player-option"
      data-focus="true"
      onClick={onSelect}
      role="radio"
      type="button"
    >
      <span className="player-option-check">{checked ? <CheckIcon /> : null}</span>
      <span>{label}</span>
    </button>
  );
}

/**
 * The post-play surface, kept presentational so its markup can be tested without a video element.
 *
 * The countdown draws the offered episode, the remaining time, and the two choices the coordinator
 * understands. The ended panel draws Replay; a film never falls through to a black screen, and
 * Cancel exists only where a follow-up can still be declined.
 */
export function PostPlayPanel({
  state,
  panelRef,
  onPlayNow,
  onCancel,
  onReplay,
}: {
  readonly state: PostPlayState;
  readonly panelRef?: RefObject<HTMLElement | null> | undefined;
  readonly onPlayNow: () => void;
  readonly onCancel: () => void;
  readonly onReplay: () => void;
}): ReactElement | null {
  if (state.phase === "hidden") return null;
  const nextUp: NextUp | null = state.nextUp;
  if (state.phase === "countdown" && nextUp !== null) {
    const seconds = Math.max(0, Math.ceil(state.remainingMs / 1000));
    return (
      <section aria-label="Next episode" className="post-play" ref={panelRef} role="dialog">
        <p className="post-play-heading">Up next</p>
        <p className="post-play-title">{nextUpLabel(nextUp)}</p>
        <p className="post-play-countdown">Playing in {seconds}s</p>
        <div className="post-play-actions">
          <button
            className="control post-play-primary"
            data-autofocus="true"
            data-focus="true"
            onClick={onPlayNow}
            type="button"
          >
            Play now
          </button>
          <button className="control" data-focus="true" onClick={onCancel} type="button">
            Cancel
          </button>
        </div>
      </section>
    );
  }
  return (
    <section aria-label="Playback finished" className="post-play" ref={panelRef} role="dialog">
      <p className="post-play-heading">Playback finished</p>
      <div className="post-play-actions">
        <button
          className="control post-play-primary"
          data-autofocus="true"
          data-focus="true"
          onClick={onReplay}
          type="button"
        >
          Replay
        </button>
      </div>
    </section>
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
