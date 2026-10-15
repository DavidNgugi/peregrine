/**
 * Filling in the posters a library has none of, one title at a time.
 *
 * **Why a backfill exists at all.** A fresh scan of a folder of files produces titles no provider has
 * described — stubs, in `PG-E5-06`'s word — and until one has a poster the grid is a wall of grey
 * rectangles. Waiting for a reader to open each title and press Play is not a plan; the library screen is
 * where the wall is, so it is where the frames are asked for.
 *
 * **Sequentially, and that is the load-bearing decision.** Every capture decodes a video, and a hundred of
 * them at once would pin the machine the reader is sitting at. One at a time also makes the progress
 * honest: `done` counts posters, not promises.
 *
 * **Once per title per session.** A title the backfill refused is remembered and not retried while the
 * screen is mounted, because a refusal is usually a property of the file rather than a race — a container
 * the platform will not seek, a codec it will not decode. Retrying would be a decode per re-render.
 *
 * **It stops when the screen does.** The capture session is closed in a `finally`, and a component that
 * unmounts mid-run closes the one it holds and processes nothing further — a listener that outlived the
 * screen would be a socket nobody can close (`SECURITY.md` T10).
 */

import {
  normalizeIpcError,
  startPosterCapture,
  stopPosterCapture,
  type IpcError,
} from "@peregrine/ipc-client";
import { useEffect, useRef, useState } from "react";

import { logUiError } from "../../../lib/error-log.js";
import { captureFrame } from "../lib/frame-capture.js";

/**
 * The next title to capture, or `null` when there is nothing left to do.
 *
 * **The rule the hook's once-per-title promise rests on, and it is a function for that reason.** This
 * harness renders to static markup with no DOM, so a hook's internals are a place no test reaches — and
 * "which title is next" is exactly the part that can be got wrong. First come, first served over the order
 * the read answered in; already-tried titles are skipped, so a title the backfill refused is not retried on
 * every re-render; a duplicate id is captured once.
 */
export function nextToCapture(
  wanted: readonly number[],
  tried: ReadonlySet<number>,
): number | null {
  for (const titleId of wanted) {
    if (!tried.has(titleId)) {
      return titleId;
    }
  }
  return null;
}

/** What a backfill has done so far. */
export interface PosterBackfill {
  /** How many posters it has stored. */
  readonly captured: number;
  /** Whether it is working right now. */
  readonly running: boolean;
  /** The last refusal, in the core's own words, or `null`. */
  readonly failed: IpcError | null;
}

/**
 * Captures a poster for every one of `titleIds` that has none.
 *
 * `titleIds` is the titles that need one, from `LibraryTitleDto.needsPoster` — the core's answer, because
 * the question needs the catalogue row and two cache lookups (`AGENTS R7`).
 */
export function usePosterBackfill(titleIds: readonly number[]): PosterBackfill {
  const [state, setState] = useState<PosterBackfill>({
    captured: 0,
    running: false,
    failed: null,
  });
  // Titles this run has already tried, and whether the screen is still mounted. Refs rather than state
  // because neither renders, and a re-render that reset the first would redo every refusal.
  const tried = useRef(new Set<number>());
  const live = useRef(true);

  // A stable key, so the effect runs when the *set* of titles changes rather than on every render: the
  // caller builds this list from a read, and a new array of the same ids is the same work.
  const key = titleIds.join(",");

  useEffect(() => {
    live.current = true;
    return () => {
      live.current = false;
    };
  }, []);

  useEffect(() => {
    const wanted = key === "" ? [] : key.split(",").map(Number);
    if (wanted.length === 0) {
      return;
    }
    let cancelled = false;

    void (async () => {
      setState((current) => ({ ...current, running: true }));
      let captured = 0;
      let failed: IpcError | null = null;

      for (
        let titleId = nextToCapture(wanted, tried.current);
        titleId !== null;
        titleId = nextToCapture(wanted, tried.current)
      ) {
        if (cancelled || !live.current) {
          break;
        }
        tried.current.add(titleId);

        try {
          const session = await startPosterCapture(titleId);
          try {
            if (session.thumbnailUrl === null) {
              continue;
            }
            const outcome = await captureFrame(session.url, null);
            if (outcome.kind === "refused") {
              continue;
            }
            const answer = await fetch(session.thumbnailUrl, {
              method: "POST",
              headers: { "Content-Type": "image/jpeg" },
              body: outcome.blob,
            });
            if (answer.ok) {
              captured += 1;
              if (live.current) {
                setState({ captured, running: true, failed: null });
              }
            }
          } finally {
            // Closed whatever happened, and by its own id: a refusal is a poster that will not arrive, not a
            // listener anybody wants left open — and a stop that did not name the session could close one a
            // later title had already opened.
            await stopPosterCapture(session.sessionId);
          }
        } catch (caught: unknown) {
          // One title's file being unreadable is not the backfill's failure: the rest of the library
          // still wants posters. The last refusal is kept so the screen can say something happened.
          failed = normalizeIpcError(caught);
          logUiError("startPosterCapture", failed);
        }
      }

      if (live.current) {
        setState({ captured, running: false, failed });
      }
    })();

    return () => {
      cancelled = true;
    };
  }, [key]);

  return state;
}
