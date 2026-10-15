/**
 * The hook layer for one title's own screen.
 *
 * **One read, because the episodes left.** The first shape read the title and every episode of it, on
 * the argument that the screen showed both. It now shows one *season* of them, chosen by a switcher,
 * which is `useSeasons`'s question — and two hooks reading the same table to answer two halves of one
 * heading was a read per title whose rows the other hook had already asked for.
 *
 * **`missing` is its own state rather than an error.** `getTitle` answers `null` for a title this
 * account may not see and for an id that names nothing, and the two are the same answer by design
 * (I37). A screen that showed a failure for either would be telling the user something the core did
 * not say.
 *
 * **The refresh is asked for, not decided here.** `refreshDue` comes from the core (AGENTS R7), and
 * this hook's part is only to act on it once per visit: the core says yes for a title no provider has
 * filled in and for one whose poster has been evicted, and no for a title whose provider had no
 * artwork at all. A screen that worked that out for itself would spend a request on every visit to a
 * posterless title, which is what `AGENTS R24` exists to prevent.
 *
 * A refresh is not a wait: the page is drawn from the library as soon as the read answers, the job
 * runs behind it, and the poster appears when it lands. A slow provider is a slow picture rather than
 * a slow screen (`ARCHITECTURE` §7 rule 2).
 */

import { eventTopics, getTitle, normalizeIpcError, refreshTitle } from "@peregrine/ipc-client";
import type {
  IpcError,
  LibraryTitleDto,
  LocalPlaybackDto,
  RootKind,
  TitleOfferDto,
} from "@peregrine/ipc-client";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { useCallback, useEffect, useRef, useState } from "react";
import { logUiError } from "../../../lib/error-log.js";
import {
  afterRouteChange,
  followRefresh,
  NOT_FOLLOWING,
  shouldAsk,
  type RefreshFollow,
} from "./refresh-follow.js";

/** One title and what the screen needs to know about the read. */
export interface TitleDetail {
  /** The title, once the read has answered. */
  readonly title: LibraryTitleDto | null;
  /**
   * What pressing Play would do, as the core classified the file's container, or `null` when the
   * library holds no copy on disk.
   *
   * The rule is the core's (`peregrine_domain::LocalPlayback`) and this carries its answer for the
   * reason every other field here does: a screen that decided it from a file extension would be
   * deciding playback in TypeScript (`AGENTS R7`).
   */
  readonly playSupport: LocalPlaybackDto | null;
  /** Where its cached poster is, as the core reported it, or `null` when there is none. */
  readonly poster: string | null;
  /** Where its cached backdrop is, or `null` when the cache holds none. */
  readonly backdrop: string | null;
  /** The provider's synopsis, when one has been written. */
  readonly overview: string | null;
  /** The provider's score, when it has one. */
  readonly rating: number | null;
  /** When it came out, as the core rendered the stored column. */
  readonly released: string | null;
  /**
   * What this title's page may offer, which the core derived rather than this hook (AGENTS R7).
   *
   * Never `null`, and the value in the states where no title was read is never drawn: the screen draws
   * the hero only in the branch a successful read produced, so the offer a user sees is always the one
   * the core computed for that title.
   */
  readonly offer: TitleOfferDto;
  /** The reads are still in flight. */
  readonly loading: boolean;
  /** The core answered that there is no such title here. */
  readonly missing: boolean;
  /** A read failed, so what is here is unknown rather than absent. */
  readonly failed: boolean;
  /** Re-reads the title. */
  readonly refresh: () => Promise<void>;
}

/** Everything the hook holds about the read itself. */
interface TitleState {
  readonly title: LibraryTitleDto | null;
  /** Carried through from the read; see [`TitleDetail::playSupport`]. */
  readonly playSupport: LocalPlaybackDto | null;
  readonly poster: string | null;
  readonly backdrop: string | null;
  readonly overview: string | null;
  readonly rating: number | null;
  readonly released: string | null;
  readonly offer: TitleOfferDto;
  readonly loading: boolean;
  readonly missing: boolean;
  readonly failed: boolean;
}

const EMPTY: TitleState = {
  title: null,
  playSupport: null,
  poster: null,
  backdrop: null,
  overview: null,
  rating: null,
  released: null,
  // Nothing has been read, so nothing is offered. The hero is not drawn in this state.
  offer: "acquire",
  loading: true,
  missing: false,
  failed: false,
};

/**
 * One title, its episodes, and the poster its provider is filling in.
 *
 * `null` is "no title route is open", which is a state the shell is in on every other screen. The
 * hook cannot be called conditionally — the rules of hooks — so the *read* is what is conditional: a
 * hook that asked the core about id zero on every route would be a request per navigation whose
 * answer nothing renders.
 */
export function useTitle(id: number | null): TitleDetail {
  const [state, setState] = useState<TitleState>(EMPTY);
  const [follow, setFollow] = useState<RefreshFollow>(NOT_FOLLOWING);
  // The id the *current* visit is for. A read that answers after the route has moved on is dropped
  // rather than applied: it would otherwise draw one title's answer under another's name, and — now
  // that a read can start a refresh — spend a provider request for a title nobody is looking at and
  // point the follow at that title's job.
  const visit = useRef<number | null>(null);
  // The id this visit has already asked a provider about. A ref rather than state because asking is a
  // side effect and not a render, and because it has to survive the re-read that follows the refresh:
  // without it, a refresh that produced no poster would be asked for again on that re-read, and every
  // one after it.
  const asked = useRef<number | null>(null);
  // `follow`, where an event that arrives before React has committed the state can still see it: a job
  // that stops faster than the render that started following it would otherwise have its one event
  // dropped, and the poster would wait for the next visit.
  const following = useRef<RefreshFollow>(NOT_FOLLOWING);

  /** Moves the follow, in the ref and in the state, so the two cannot drift. */
  const applyFollow = useCallback((next: RefreshFollow): void => {
    following.current = next;
    setFollow(next);
  }, []);

  /**
   * Starts the refresh job and follows it.
   *
   * **A failure to start is reported and not shown as the title's own.** The page already has the
   * library's answer, and a provider that could not be reached is a missing poster rather than a
   * title that could not be read: replacing the page with an error would tell the user the core said
   * something it did not. The job journal is where the reason lives (`PG-E25-07`).
   */
  const startRefresh = useCallback(
    async (titleId: number, kind: RootKind): Promise<void> => {
      try {
        const started = await refreshTitle(titleId, kind);
        applyFollow({ ...following.current, jobId: started.jobId });
      } catch (caught: unknown) {
        logUiError("refreshTitle", normalizeIpcError(caught));
      }
    },
    [applyFollow],
  );

  const read = useCallback(async (): Promise<void> => {
    // What this route change leaves of the memory, before anything else looks at it: a route that is
    // not a title forgets, so coming back to a title is a new visit that may ask again.
    asked.current = afterRouteChange(asked.current, id);
    if (id === null) {
      // Nothing to read and nothing to say: the screen that would show it is not rendered. A refresh
      // still in flight is left alone: it belongs to the title just left, and it reports its own
      // completion.
      setState(EMPTY);
      return;
    }
    try {
      const answer = await getTitle(id);
      if (visit.current !== id) {
        // The route moved on while this read was in flight; its answer is about a page that is no
        // longer open, and acting on it would spend a request for a title nobody is looking at.
        return;
      }
      if (answer === null) {
        setState({
          title: null,
          playSupport: null,
          poster: null,
          backdrop: null,
          overview: null,
          rating: null,
          released: null,
          offer: "acquire",
          loading: false,
          missing: true,
          failed: false,
        });
        return;
      }
      setState({
        title: answer.title,
      playSupport: answer.playSupport,
        // The poster is the read's own row rather than a field beside it: the title the core answered
        // with carries the cached path, so a page and a tile cannot disagree about what there is to draw.
        poster: answer.title.poster,
        backdrop: answer.backdrop,
        overview: answer.overview,
        rating: answer.rating,
        released: answer.released,
        offer: answer.offer,
        loading: false,
        missing: false,
        failed: false,
      });
      if (shouldAsk(asked.current, id, answer.refreshDue)) {
        asked.current = id;
        await startRefresh(id, answer.title.kind);
      }
    } catch (caught: unknown) {
      if (visit.current !== id) {
        return;
      }
      const error: IpcError = normalizeIpcError(caught);
      logUiError("getTitle", error);
      setState({
        title: null,
        playSupport: null,
        poster: null,
        backdrop: null,
        overview: null,
        rating: null,
        released: null,
        offer: "acquire",
        loading: false,
        missing: false,
        failed: true,
      });
    }
  }, [id, startRefresh]);

  // The refresh this screen asked for, followed until it stops. Only the job this hook started is
  // followed: another screen's job is that screen's business.
  //
  // **A job that fails publishes no terminal phase over this topic**, and that is a gap in the event
  // contract rather than something this hook can paper over: a job reports its own success, and a
  // failure is the scheduler's outcome, which nothing publishes yet (`PG-E6-01` owns it). Nothing on
  // screen is rendered from the follow, so the visible result is a poster that does not appear until
  // the next visit asks again — never a screen waiting for a stop that cannot arrive.
  useEffect(() => {
    let cancelled = false;
    let unlisten: UnlistenFn | null = null;

    void listen<unknown>(eventTopics.jobState.name, (event) => {
      // `followRefresh` answers the value it was handed when the event is not this job's stop, so an
      // event about another screen's job costs a reference comparison and no render.
      const next = followRefresh(following.current, event.payload);
      if (next !== following.current) {
        applyFollow(next);
      }
    }).then((release) => {
      if (cancelled) {
        // The effect was cleaned up before the listener registered, which happens when a screen
        // mounts and unmounts inside one tick; releasing it now is what keeps a visit from leaking a
        // listener.
        release();
      } else {
        unlisten = release;
      }
    });

    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [applyFollow]);

  // One effect for both triggers: a new route gives the callback a new identity, and a refresh
  // stopping moves the count. Two separate effects would read twice on the navigation that follows a
  // refresh. The visit is recorded here rather than in the read, because this is the moment the route
  // the screen is for becomes the route the reads are about.
  useEffect(() => {
    visit.current = id;
    void read();
  }, [id, read, follow.finished]);

  return { ...state, refresh: read };
}
