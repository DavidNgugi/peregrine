/**
 * The skip markers a session opens with, and the small pure coordinator between a finished
 * episode and the one after it.
 *
 * **Markers are read, never inferred.** The session arrives with the cache's answer; a refresh is
 * started after it opens, and a `player://markers` event only says the cache changed. The event
 * payload carries the subject and a count, not marker windows, so a subscriber cannot render a
 * second, unvalidated copy of the cache.
 *
 * **The coordinator is a projection of `@peregrine/player-ui`'s machine.** `advance` owns the
 * countdown, the cancel, and the exactly-one start; this module only keeps the next-up the session
 * offered alongside it and turns a `start-next` command into the one call the shell must make.
 * Keeping that split here means a React component never re-derives a rule the package already
 * states.
 */

import {
  commands,
  eventTopics,
  normalizeIpcError,
  unwrap,
  type PlaybackSessionDto,
} from "@peregrine/ipc-client";
import {
  advance,
  type NextUp,
  type PlayerMarker,
  type PostPlayCommand,
  type PostPlayEvent,
  type PostPlayState,
} from "@peregrine/player-ui";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { useEffect, useRef, useState } from "react";

import { logUiError } from "../../../lib/error-log.js";

/** One marker exactly as the session carries it. */
type SessionMarker = PlaybackSessionDto["markers"][number];

/** The subject a marker read or refresh names. */
type MarkerSubject = Parameters<typeof commands.markersForPlayback>[0];

/** The shape the backend publishes on `player://markers`; a count is all it carries. */
interface MarkerEventPayload {
  readonly titleId: number;
  readonly episodeId: number | null;
}

function markerSubject(session: PlaybackSessionDto): MarkerSubject {
  return {
    titleId: session.titleId,
    episodeId: session.episodeId,
    durationMs: session.durationMs,
  };
}

function markerEventPayload(payload: unknown): MarkerEventPayload | null {
  if (typeof payload !== "object" || payload === null) {
    return null;
  }
  const record = payload as Record<string, unknown>;
  const titleId = record["titleId"];
  const episodeId = record["episodeId"];
  if (typeof titleId !== "number" || !Number.isFinite(titleId)) {
    return null;
  }
  if (episodeId !== null && typeof episodeId !== "number") {
    return null;
  }
  return { titleId, episodeId };
}

/**
 * Whether an event is about the subject this screen is playing.
 *
 * A missing or foreign payload is not this screen's news. The event is a trigger to re-read, so
 * ignoring one costs a refresh that arrives through the next event instead.
 */
export function isMarkerEventFor(
  payload: unknown,
  titleId: number,
  episodeId: number | null,
): boolean {
  const event = markerEventPayload(payload);
  return event !== null && event.titleId === titleId && event.episodeId === episodeId;
}

/**
 * The session's markers in the shape the overlay draws.
 *
 * `MarkerDto` has no id of its own; `PlayerMarker` needs one, and the start is the only identity a
 * marker actually has. The overlay never uses it as a React key, so a duplicate start is harmless.
 */
export function markersFromSession(markers: readonly SessionMarker[]): readonly PlayerMarker[] {
  return markers.map((marker) => ({
    id: marker.startMs,
    kind: marker.kind,
    startMs: marker.startMs,
    endMs: marker.endMs,
    source: marker.source,
    confidence: marker.confidence,
  }));
}

/** The episode after this one, as the surface's post-play machine names it. */
export function nextUpFromSession(session: PlaybackSessionDto | null): NextUp | null {
  if (session === null || session.nextEpisode === null) {
    return null;
  }
  const next = session.nextEpisode;
  return {
    titleId: session.titleId,
    seasonNumber: next.seasonNumber,
    episodeNumber: next.episodeNumber,
    episodeId: next.episodeId,
    name: next.name,
  };
}

/**
 * The session's markers, replaced when the backend says the cache changed.
 *
 * The initial value is the session's answer, so a screen that opens with a cached intro draws its
 * skip control immediately. The refresh runs once per session rather than once per render: a
 * provider call is the one thing in this path that is not local, and a re-render is not a reason to
 * ask again.
 */
export function usePlayerMarkers(session: PlaybackSessionDto | null): readonly PlayerMarker[] {
  const [markers, setMarkers] = useState<readonly PlayerMarker[]>(() =>
    session === null ? [] : markersFromSession(session.markers),
  );
  // The session whose refresh was already asked for. A ref because StrictMode mounts the effect
  // twice, and the second mount must not be a second provider call.
  const refreshedFor = useRef<number | null>(null);

  useEffect(() => {
    if (session === null) {
      setMarkers([]);
      return;
    }
    setMarkers(markersFromSession(session.markers));
    if (refreshedFor.current === session.sessionId) {
      return;
    }
    refreshedFor.current = session.sessionId;
    void unwrap(commands.refreshPlaybackMarkers(markerSubject(session))).catch(
      (caught: unknown) => {
        // A refresh that cannot start is not fatal: the session's own markers are already drawn,
        // and the next opening retries. There is nothing on this screen a viewer could do about it.
        logUiError("player.refreshMarkers", normalizeIpcError(caught));
      },
    );
  }, [session]);

  useEffect(() => {
    if (session === null) {
      return;
    }
    const subject = markerSubject(session);
    let live = true;
    let unlisten: UnlistenFn | null = null;

    void listen<unknown>(eventTopics.playerMarkers.name, (event) => {
      if (!isMarkerEventFor(event.payload, subject.titleId, subject.episodeId)) {
        return;
      }
      // The event is a trigger and never the answer: re-read through the command the screen
      // already uses, so the overlay cannot draw a shape the core would not serve.
      void unwrap(commands.markersForPlayback(subject))
        .then((answer) => {
          if (live) {
            setMarkers(markersFromSession(answer));
          }
        })
        .catch((caught: unknown) => {
          logUiError("player.readMarkers", normalizeIpcError(caught));
        });
    }).then((stop) => {
      if (live) {
        unlisten = stop;
      } else {
        stop();
      }
    });

    return () => {
      live = false;
      unlisten?.();
    };
  }, [session]);

  return markers;
}

/** What one post-play transition asks the shell to do, with the offer it was about. */
export interface PostPlayStep {
  readonly state: PostPlayState;
  readonly command: PostPlayCommand | null;
  /** The next episode when the command is `start-next`; `advance` has cleared it from the state. */
  readonly start: NextUp | null;
}

/**
 * One transition, retaining the next-up the coordinator deliberately clears on a start.
 *
 * `advance` moves to `hidden` when it issues `start-next`, which drops `nextUp` as part of its
 * cancel-safety guarantee. The caller still needs the episode that command was about, so it is
 * captured before the transition rather than read back from a state that no longer has it.
 */
export function stepPostPlay(state: PostPlayState, event: PostPlayEvent): PostPlayStep {
  const offered = state.nextUp;
  const outcome = advance(state, event);
  return {
    state: outcome.state,
    command: outcome.command,
    start: outcome.command === "start-next" ? offered : null,
  };
}

/** How the player opens a follow-up: `Playback.open`, which replaces the current session. */
type EpisodeOpener = (
  titleId: number,
  season: number | null,
  episode: number | null,
) => Promise<void>;

/**
 * Opens the next episode, once, with the identity the core already resolved.
 *
 * **The prefetch outcome is not consulted.** A follow-up whose warm-up failed is still the episode
 * the viewer chose, and `open` loads it normally; refusing here would turn a cache miss into a
 * dead end.
 */
export function startNextEpisode(next: NextUp, open: EpisodeOpener): void {
  void open(next.titleId, next.seasonNumber, next.episodeNumber);
}
