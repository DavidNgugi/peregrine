/**
 * The shared player contract: what every engine reports to the surface, and the pure projection
 * that turns a command or an engine event into the next snapshot.
 *
 * The backend owns the real session (FLOWS SM3); this module owns the much smaller shape the
 * HTML overlay draws and the adapter must honour. The distinction is deliberate: the surface
 * cannot depend on DOM playback, and a test can drive every transition without a video element.
 */

/** The seven states a playback session can be drawn in. */
export type PlayerState =
  "idle" | "loading" | "playing" | "paused" | "buffering" | "ended" | "failed";

/** One audio or subtitle stream offered by the current media. */
export interface PlayerTrack {
  readonly id: string;
  readonly label: string;
  readonly language: string | null;
  readonly selected: boolean;
  /** A track the container marks as mandatory when available. */
  readonly forced?: boolean;
  /** Subtitles for the deaf and hard of hearing. */
  readonly sdh?: boolean;
}

/** A skip window resolved by the backend, in media time. */
export interface PlayerMarker {
  readonly id: number;
  readonly kind: "intro" | "recap" | "credits" | "preview";
  readonly startMs: number;
  /** `null` only where the marker runs to the end of the media, which credits may. */
  readonly endMs: number | null;
  readonly source: string;
  /** Whole percent, 0 through 100. */
  readonly confidence: number;
}

/** The episode the surface offers after the current one. */
export interface NextUp {
  readonly titleId: number;
  readonly seasonNumber: number;
  readonly episodeNumber: number;
  readonly episodeId: number;
  readonly name: string | null;
}

/** What the current engine can do, so the surface does not draw a dead control. */
export interface PlayerCapabilities {
  readonly pip: boolean;
  readonly fullscreen: boolean;
  readonly tracks: boolean;
  readonly seek: boolean;
}

/** Everything the overlay draws, in one comparable value. */
export interface PlayerSnapshot {
  readonly state: PlayerState;
  readonly positionMs: number;
  readonly durationMs: number | null;
  readonly paused: boolean;
  readonly buffering: boolean;
  readonly volume: number;
  readonly rate: number;
  readonly audioTracks: readonly PlayerTrack[];
  readonly subtitleTracks: readonly PlayerTrack[];
  readonly markers: readonly PlayerMarker[];
  readonly nextUp: NextUp | null;
  readonly error: string | null;
  readonly capabilities: PlayerCapabilities;
}

/**
 * A request to open media.
 *
 * `url` is the only identity the surface needs; the backend resolves it and reports duration,
 * tracks, and markers through {@link PlayerEvent} once the container is probed. A blank URL is
 * refused by {@link applyPlayerCommand} rather than handing the engine an unopenable source.
 */
export interface PlayerLoad {
  readonly url: string;
  /** Resume position. An absent, negative, or non-finite value starts at zero. */
  readonly startPositionMs?: number;
}

/** An intent the surface sends to an engine. */
export type PlayerCommand =
  | ({ readonly type: "load" } & PlayerLoad)
  | { readonly type: "play" }
  | { readonly type: "pause" }
  | { readonly type: "seek"; readonly positionMs: number }
  | { readonly type: "setRate"; readonly rate: number }
  | { readonly type: "selectAudio"; readonly trackId: string }
  /** `null` turns subtitles off. */
  | { readonly type: "selectSubtitle"; readonly trackId: string | null }
  | { readonly type: "setVolume"; readonly volume: number }
  | { readonly type: "stop" }
  | { readonly type: "retry" };

/**
 * Something the engine observed.
 *
 * These are the facts the backend owns; the surface never infers them from the video element,
 * because the mpv adapter and the browser adapter do not expose the same properties.
 */
export type PlayerEvent =
  | { readonly type: "loading" }
  | {
      readonly type: "loaded";
      readonly durationMs: number;
      readonly audioTracks: readonly PlayerTrack[];
      readonly subtitleTracks: readonly PlayerTrack[];
      readonly markers: readonly PlayerMarker[];
      readonly nextUp: NextUp | null;
    }
  | { readonly type: "playing" }
  | { readonly type: "paused" }
  | { readonly type: "buffering" }
  | { readonly type: "position"; readonly positionMs: number }
  | { readonly type: "ended" }
  | { readonly type: "failed"; readonly error: string }
  | { readonly type: "nextUp"; readonly nextUp: NextUp | null }
  | {
      readonly type: "tracks";
      readonly audioTracks: readonly PlayerTrack[];
      readonly subtitleTracks: readonly PlayerTrack[];
    }
  | { readonly type: "markers"; readonly markers: readonly PlayerMarker[] }
  | { readonly type: "volume"; readonly volume: number }
  | { readonly type: "rate"; readonly rate: number }
  | { readonly type: "capabilities"; readonly capabilities: PlayerCapabilities };

/**
 * The port every playback implementation satisfies.
 *
 * `snapshot` is synchronous because the surface calls it during render; an engine that has not
 * finished connecting reports {@link initialPlayerSnapshot} rather than making the caller wait.
 */
export interface PlayerEngine {
  snapshot(): PlayerSnapshot;
  subscribe(listener: (snapshot: PlayerSnapshot) => void): () => void;
  command(command: PlayerCommand): void;
  destroy(): void;
}

const MIN_RATE = 0.25;
const MAX_RATE = 4;
const MIN_VOLUME = 0;
const MAX_VOLUME = 1;

/** A fresh session, before any media has been opened. */
export function initialPlayerSnapshot(): PlayerSnapshot {
  return {
    state: "idle",
    positionMs: 0,
    durationMs: null,
    paused: false,
    buffering: false,
    volume: 1,
    rate: 1,
    audioTracks: [],
    subtitleTracks: [],
    markers: [],
    nextUp: null,
    error: null,
    capabilities: { pip: false, fullscreen: false, tracks: false, seek: false },
  };
}

type SnapshotChanges = Partial<Omit<PlayerSnapshot, "state" | "paused" | "buffering">>;

/**
 * Rebuilds a snapshot for `next`, keeping the three state-derived fields in lockstep.
 *
 * `paused` and `buffering` are projections rather than independent facts: two sources of the
 * same truth drift the moment one transition forgets one of them.
 */
function transition(
  snapshot: PlayerSnapshot,
  state: PlayerState,
  changes: SnapshotChanges = {},
): PlayerSnapshot {
  return {
    ...snapshot,
    ...changes,
    state,
    paused: state === "paused",
    buffering: state === "buffering",
  };
}

function isLive(state: PlayerState): boolean {
  return state === "loading" || state === "playing" || state === "paused" || state === "buffering";
}

function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value));
}

/**
 * A duration the surface can draw a timeline against, or `null` when it has no bound.
 *
 * A live stream reports `Infinity` and a probe that failed reports garbage; both are "unknown",
 * not a very long film.
 */
function normaliseDuration(durationMs: number | null): number | null {
  if (durationMs === null || !Number.isFinite(durationMs) || durationMs < 0) {
    return null;
  }
  return durationMs;
}

function clampPosition(positionMs: number, durationMs: number | null): number {
  if (!Number.isFinite(positionMs)) {
    return 0;
  }
  const clamped = Math.max(0, positionMs);
  const upper =
    durationMs !== null && Number.isFinite(durationMs) && durationMs >= 0 ? durationMs : null;
  return upper === null ? clamped : Math.min(clamped, upper);
}

function errorText(error: string): string {
  const trimmed = error.trim();
  return trimmed.length > 0 ? trimmed : "Playback failed.";
}

function copyTracks(tracks: readonly PlayerTrack[]): PlayerTrack[] {
  return tracks.map((track) => ({ ...track }));
}

function copyMarkers(markers: readonly PlayerMarker[]): PlayerMarker[] {
  return markers.map((marker) => ({ ...marker }));
}

/**
 * The next snapshot for a command, or the same snapshot when the command cannot apply.
 *
 * A command is applied to the snapshot the surface already holds; it never reads engine events
 * or a DOM node, so a control that renders from this value cannot disagree with one that calls
 * the engine directly.
 */
export function applyPlayerCommand(state: PlayerSnapshot, command: PlayerCommand): PlayerSnapshot {
  switch (command.type) {
    case "load": {
      if (command.url.trim().length === 0) {
        return state;
      }
      const requested = command.startPositionMs ?? 0;
      const positionMs = Number.isFinite(requested) ? Math.max(0, requested) : 0;
      return {
        ...state,
        state: "loading",
        positionMs,
        durationMs: null,
        paused: false,
        buffering: false,
        audioTracks: [],
        subtitleTracks: [],
        markers: [],
        nextUp: null,
        error: null,
      };
    }
    case "play":
      return isLive(state.state) ? transition(state, "playing") : state;
    case "pause":
      return isLive(state.state) ? transition(state, "paused") : state;
    case "seek": {
      if (!isLive(state.state) || !Number.isFinite(command.positionMs)) {
        return state;
      }
      const positionMs = clampPosition(command.positionMs, state.durationMs);
      return positionMs === state.positionMs ? state : { ...state, positionMs };
    }
    case "setRate": {
      if (!Number.isFinite(command.rate)) {
        return state;
      }
      const rate = clamp(command.rate, MIN_RATE, MAX_RATE);
      return rate === state.rate ? state : { ...state, rate };
    }
    case "selectAudio": {
      if (!isLive(state.state)) {
        return state;
      }
      const audioTracks = state.audioTracks.map((track) => ({
        ...track,
        selected: track.id === command.trackId,
      }));
      return audioTracks.some((track) => track.selected) ? { ...state, audioTracks } : state;
    }
    case "selectSubtitle": {
      if (!isLive(state.state)) {
        return state;
      }
      if (command.trackId === null) {
        return {
          ...state,
          subtitleTracks: state.subtitleTracks.map((track) => ({ ...track, selected: false })),
        };
      }
      const subtitleTracks = state.subtitleTracks.map((track) => ({
        ...track,
        selected: track.id === command.trackId,
      }));
      return subtitleTracks.some((track) => track.selected) ? { ...state, subtitleTracks } : state;
    }
    case "setVolume": {
      if (!Number.isFinite(command.volume)) {
        return state;
      }
      const volume = clamp(command.volume, MIN_VOLUME, MAX_VOLUME);
      return volume === state.volume ? state : { ...state, volume };
    }
    case "stop": {
      // Volume, rate, and capabilities belong to the engine, not the media, so stopping a film
      // does not reset preferences the next load should inherit.
      return {
        ...initialPlayerSnapshot(),
        volume: state.volume,
        rate: state.rate,
        capabilities: state.capabilities,
      };
    }
    case "retry":
      return state.state === "failed" ? transition(state, "loading", { error: null }) : state;
  }
}

/**
 * The next snapshot for an engine event, or the same snapshot when the event is out of session.
 *
 * Terminal states absorb state changes, but metadata may still arrive: a credits marker resolved
 * after the stream ended is exactly the fact the post-play surface needs. `loading` is the one
 * event that starts a new session, and `idle` refuses everything because no engine owns it.
 */
export function reducePlayerEvent(state: PlayerSnapshot, event: PlayerEvent): PlayerSnapshot {
  switch (event.type) {
    case "loading":
      return transition(state, "loading", { error: null });
    case "loaded": {
      if (state.state === "idle") {
        return state;
      }
      const durationMs = normaliseDuration(event.durationMs);
      return transition(state, state.state === "loading" ? "paused" : state.state, {
        durationMs,
        positionMs: clampPosition(state.positionMs, durationMs),
        audioTracks: copyTracks(event.audioTracks),
        subtitleTracks: copyTracks(event.subtitleTracks),
        markers: copyMarkers(event.markers),
        nextUp: event.nextUp,
        error: null,
      });
    }
    case "playing":
      return isLive(state.state) ? transition(state, "playing") : state;
    case "paused":
      return isLive(state.state) ? transition(state, "paused") : state;
    case "buffering":
      return isLive(state.state) ? transition(state, "buffering") : state;
    case "position": {
      if (!isLive(state.state) || !Number.isFinite(event.positionMs)) {
        return state;
      }
      const positionMs = clampPosition(event.positionMs, state.durationMs);
      return positionMs === state.positionMs ? state : { ...state, positionMs };
    }
    case "ended": {
      if (!isLive(state.state)) {
        return state;
      }
      return transition(state, "ended", {
        positionMs: state.durationMs ?? state.positionMs,
      });
    }
    case "failed":
      return state.state === "idle" || state.state === "failed"
        ? state
        : transition(state, "failed", { error: errorText(event.error) });
    case "nextUp":
      return state.state === "idle" ? state : { ...state, nextUp: event.nextUp };
    case "tracks":
      return state.state === "idle"
        ? state
        : {
            ...state,
            audioTracks: copyTracks(event.audioTracks),
            subtitleTracks: copyTracks(event.subtitleTracks),
          };
    case "markers":
      return state.state === "idle" ? state : { ...state, markers: copyMarkers(event.markers) };
    case "volume": {
      if (!Number.isFinite(event.volume)) {
        return state;
      }
      const volume = clamp(event.volume, MIN_VOLUME, MAX_VOLUME);
      return volume === state.volume ? state : { ...state, volume };
    }
    case "rate": {
      if (!Number.isFinite(event.rate)) {
        return state;
      }
      const rate = clamp(event.rate, MIN_RATE, MAX_RATE);
      return rate === state.rate ? state : { ...state, rate };
    }
    case "capabilities":
      return { ...state, capabilities: event.capabilities };
  }
}

/**
 * The marker window containing `positionMs`, preferring the earliest start.
 *
 * Windows are half-open — a start is inside, an end is not — because a marker that ends where the
 * next begins must not claim a frame of the next one. `endMs: null` is an open window and runs
 * as far as the media does, which is the credits case. An overlap resolves to the earliest start,
 * so a preview detected inside the credits cannot replace the credits window.
 */
export function activeMarker(
  markers: readonly PlayerMarker[],
  positionMs: number,
): PlayerMarker | null {
  if (!Number.isFinite(positionMs)) {
    return null;
  }
  let active: PlayerMarker | null = null;
  for (const marker of markers) {
    if (!Number.isFinite(marker.startMs) || positionMs < marker.startMs) {
      continue;
    }
    if (marker.endMs !== null) {
      if (!Number.isFinite(marker.endMs) || positionMs >= marker.endMs) {
        continue;
      }
    }
    if (active === null || marker.startMs < active.startMs) {
      active = marker;
    }
  }
  return active;
}
