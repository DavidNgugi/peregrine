/**
 * The client half of the LAN contract (ADR 0028).
 *
 * Every call is same-origin and carries the pairing cookie the listener set, which is HttpOnly and
 * therefore not readable here -- the browser attaches it, and that is the whole reason the token is
 * a cookie rather than a string this module could leak into a log.
 */

/** A failure the listener answered with, carrying its stable code. */
export class ApiError extends Error {
  constructor(
    readonly status: number,
    readonly code: string,
    message: string,
  ) {
    super(message);
    this.name = "ApiError";
  }
}

/** One title the library holds, as the grid draws it. */
export interface LibraryRow {
  readonly id: number;
  readonly name: string;
  readonly kind: string;
  readonly poster: string | null;
  readonly rating: number | null;
  readonly overview: string | null;
  readonly items: number;
  readonly addedAt: number;
}

/** One title as its page draws it. */
export interface Progress {
  readonly episodeId: number | null;
  readonly positionMs: number;
  readonly durationMs: number | null;
  readonly percent: number;
  readonly watched: boolean;
}

export interface Tile {
  readonly id: number;
  readonly name: string;
  readonly kind: string;
  readonly year: number | null;
  readonly poster: string | null;
  readonly backdrop: string | null;
  readonly rating: number | null;
  readonly overview: string | null;
  readonly playable: boolean;
  readonly percent: number | null;
  readonly positionMs: number;
}

export interface Rail {
  readonly id: string;
  readonly title: string;
  readonly items: Tile[];
}

export interface HomeData {
  readonly hero: Tile | null;
  readonly rails: Rail[];
  readonly heroes: Tile[];
  readonly state: string;
}

export interface BrowseData {
  readonly items: Tile[];
  readonly hasMore: boolean;
  readonly source: "provider" | "library";
}

export interface Genre {
  readonly id: number;
  readonly name: string;
}

export interface Download {
  readonly id: number;
  readonly titleId: number;
  readonly title: string | null;
  readonly kind: string | null;
  readonly poster: string | null;
  readonly state: string;
  readonly progressPermille: number;
  readonly downloadedBytes: number;
  readonly totalBytes: number | null;
  readonly rateBps: number;
  readonly etaSeconds: number | null;
  readonly seasonNumber: number | null;
  readonly episodeNumber: number | null;
  readonly error: string | null;
}

export interface Season {
  readonly number: number;
  readonly name: string | null;
  readonly episodeCount: number | null;
  readonly poster: string | null;
}

export interface Episode {
  readonly seasonNumber: number;
  readonly episodeNumber: number;
  readonly name: string | null;
  readonly overview: string | null;
  readonly still: string | null;
  readonly runtimeMinutes: number | null;
  readonly rating: number | null;
  readonly airDate: string | null;
  readonly files: number;
  readonly percent: number;
  readonly positionMs: number;
  readonly watched: boolean;
  readonly playable: boolean;
}

export interface Title {
  readonly id: number;
  readonly name: string;
  readonly kind: string;
  readonly poster: string | null;
  readonly backdrop: string | null;
  readonly overview: string | null;
  readonly rating: number | null;
  readonly released: string | null;
  readonly items: number;
  readonly files: number;
  readonly playable: boolean;
  readonly progress: Progress | null;
  readonly inLibrary: boolean;
  readonly seasons: Season[];
  readonly episodes: Episode[];
  readonly similar: Tile[];
  readonly similarGenres: Genre[];
  readonly refreshing: boolean;
}

/** A pairing request as the TV creates it. */
export interface PairOffer {
  readonly pairId: string;
  readonly code: string;
  readonly expiresAtMs: number;
}

/** The state a pairing request is in. */
export interface PairStatus {
  readonly state: "pending" | "approved" | "denied" | "unknown";
  readonly token: string | null;
  readonly name: string | null;
  readonly expiresAtMs: number | null;
}

/** A playback URL the listener minted for this device. */
export interface PlaybackTicket {
  readonly kind: "direct" | "hls";
  readonly url: string;
  readonly mime: string;
  readonly startPositionMs: number;
  readonly durationMs: number | null;
  readonly episodeId: number | null;
}

async function request<T>(
  path: string,
  method: "GET" | "POST" = "GET",
  body?: unknown,
  keepalive = false,
): Promise<T> {
  const init: RequestInit = { method, credentials: "same-origin", keepalive };
  if (body !== undefined) {
    init.headers = { "content-type": "application/json" };
    init.body = JSON.stringify(body);
  }
  const response = await fetch(path, init);
  if (!response.ok) {
    let code = "error";
    let message = "the request was refused";
    try {
      const answer = (await response.json()) as { code?: unknown; message?: unknown };
      if (typeof answer.code === "string") code = answer.code;
      if (typeof answer.message === "string") message = answer.message;
    } catch {
      // A refusal without a JSON body is still a refusal; the status is the answer.
    }
    throw new ApiError(response.status, code, message);
  }
  if (response.status === 204) {
    return undefined as T;
  }
  return (await response.json()) as T;
}

/** The three reads and the two writes this client needs. */
export const api = {
  home: (): Promise<HomeData> => request<HomeData>("/api/home"),
  browse: (
    kind: "movie" | "show",
    page: number,
    sort: string,
    genre?: number,
  ): Promise<BrowseData> =>
    request<BrowseData>(
      "/api/browse?kind=" +
        kind +
        "&page=" +
        String(page) +
        "&sort=" +
        encodeURIComponent(sort) +
        (genre === undefined ? "" : "&genre=" + String(genre)),
    ),
  search: (query: string, kind: "movie" | "show"): Promise<Tile[]> =>
    request<Tile[]>("/api/search?q=" + encodeURIComponent(query) + "&kind=" + kind),
  genres: (kind: "movie" | "show"): Promise<Genre[]> =>
    request<Genre[]>("/api/genres?kind=" + kind),
  watchlist: (): Promise<Tile[]> => request<Tile[]>("/api/watchlist"),
  downloads: (): Promise<Download[]> => request<Download[]>("/api/downloads"),
  stopPlayback: (url: string): Promise<void> =>
    request<void>("/api/playback/stop", "POST", { url }, true),
  library: (): Promise<LibraryRow[]> => request<LibraryRow[]>("/api/library?limit=100"),
  title: (id: number, season?: number): Promise<Title> =>
    request<Title>(
      "/api/title/" + String(id) + (season === undefined ? "" : "?season=" + String(season)),
    ),
  pair: (name: string, platform: string): Promise<PairOffer> =>
    request<PairOffer>("/api/pair", "POST", { name, platform }),
  pairStatus: (pairId: string): Promise<PairStatus> =>
    request<PairStatus>("/api/pair/" + encodeURIComponent(pairId)),
  playback: (
    id: number,
    positionMs = 0,
    seasonNumber?: number,
    episodeNumber?: number,
    episodeId?: number,
  ): Promise<PlaybackTicket> =>
    request<PlaybackTicket>("/api/playback/" + String(id), "POST", {
      positionMs,
      seasonNumber: seasonNumber ?? null,
      episodeNumber: episodeNumber ?? null,
      episodeId: episodeId ?? null,
    }),
  progress: (
    titleId: number,
    episodeId: number | null,
    positionMs: number,
    durationMs: number | null,
    keepalive = false,
  ): Promise<void> =>
    request<void>(
      "/api/progress",
      "POST",
      { titleId, episodeId, positionMs, durationMs },
      keepalive,
    ),
};
