/**
 * Starting and stopping a playback session, and the state a screen draws from it.
 *
 * **The whole of this hook is a lifetime.** A session owns a loopback listener, and SECURITY.md T10
 * asks for a socket that dies with the session — so the hook that opens one has to close it, and the
 * cleanup is the part that matters rather than an afterthought. `stopPlayback` runs on unmount, which
 * is what a reader pressing Back, opening another title, or closing the window all reduce to.
 *
 * **`startPlayback` is a command and not a job**, and that is deliberate: it binds a socket, checks one
 * file, and answers a URL — no work a reader waits on. `stopPlayback` is idempotent by design, so the
 * cleanup cannot fail into a stuck surface.
 *
 * **The URL is never rebuilt, and the token in it is never exposed to anything else.** It arrives from
 * the core, it is handed straight to the media element, and nothing here interpolates it into a second
 * place it could be logged from.
 *
 * **Every stop names the session it means, and that is a fix rather than a flourish.** React's
 * `StrictMode` runs this hook's effect twice — mount, cleanup, mount — so the first mount's cleanup is in
 * flight at the same time as the second mount's open. A stop that closed "whatever is playing" therefore
 * closed the session the element was already reading, and the film died after one request behind a black
 * rectangle. The core refuses a stop that does not name the session it is holding, so a stale cleanup
 * cannot close anything.
 */

import {
  normalizeIpcError,
  startPlayback,
  stopPlayback,
  type IpcError,
  type PlaybackSessionDto,
} from "@peregrine/ipc-client";
import { useCallback, useEffect, useRef, useState } from "react";
import { logUiError } from "../../../lib/error-log.js";

/** What the player screen draws. */
export interface Playback {
  /** The open session, or `null` while it is being opened and after a refusal. */
  readonly session: PlaybackSessionDto | null;
  /** The session is being opened. */
  readonly starting: boolean;
  /** Why it could not be opened, in the core's own words. */
  readonly failed: IpcError | null;
  /** Opens a session for a title, replacing whatever this hook had open. */
  readonly open: (titleId: number) => Promise<void>;
  /** Closes whatever is open. Idempotent, so a caller may always call it. */
  readonly close: () => Promise<void>;
}

/**
 * One playback session, opened on request and closed when this stops being mounted.
 *
 * The title the screen was opened for is the `titleId` it is asked about rather than a value captured
 * at mount, because a route change can point the same screen at another title and the effect must
 * reopen rather than keep playing the first.
 */
export function usePlayback(titleId: number): Playback {
  const [state, setState] = useState<{
    session: PlaybackSessionDto | null;
    starting: boolean;
    failed: IpcError | null;
  }>({ session: null, starting: false, failed: null });

  // The session this hook opened, which is what every stop names. A ref rather than state because nothing
  // renders from it, and because the cleanup must read the value at the moment it runs rather than the one
  // it was created with.
  const opened = useRef<number | null>(null);
  // How many opens have been asked for. StrictMode's second mount must win, and an answer to an earlier
  // ask must be closed by its own id rather than adopted.
  const attempt = useRef(0);
  // Whether the component is still mounted: an answer that arrives after unmount must not set state,
  // and — more importantly — a session opened by an answer that arrived after unmount must be closed
  // rather than leaked.
  const live = useRef(true);

  const close = useCallback(async (): Promise<void> => {
    const session = opened.current;
    opened.current = null;
    setState((current) =>
      current.session === null ? current : { ...current, session: null },
    );
    if (session === null) {
      return;
    }
    try {
      await stopPlayback(session);
    } catch (caught: unknown) {
      // A failure to stop is worth a log line and nothing else: the core closes the listener when the
      // session is replaced or dropped, so the socket is not left open by this, and a screen that
      // showed an error for it would be reporting a cleanup it cannot act on.
      logUiError("stopPlayback", normalizeIpcError(caught));
    }
  }, []);

  const open = useCallback(async (forTitle: number): Promise<void> => {
    const generation = attempt.current + 1;
    attempt.current = generation;
    setState({ session: null, starting: true, failed: null });
    try {
      const session = await startPlayback(forTitle);
      if (!live.current || attempt.current !== generation) {
        // Either the screen went away while the core was binding a listener, or a later mount has already
        // asked for its own. Nothing will unmount again for this answer, so this is the only place that can
        // close it — and by its own id, so it cannot close the session the later mount opened.
        await stopPlayback(session.sessionId);
        return;
      }
      opened.current = session.sessionId;
      setState({ session, starting: false, failed: null });
    } catch (caught: unknown) {
      if (!live.current) {
        return;
      }
      const error = normalizeIpcError(caught);
      logUiError("startPlayback", error);
      setState({ session: null, starting: false, failed: error });
    }
  }, []);

  useEffect(() => {
    live.current = true;
    void open(titleId);
    return () => {
      live.current = false;
      void close();
    };
  }, [titleId, open, close]);

  return { ...state, open, close };
}
