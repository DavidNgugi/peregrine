/**
 * Posting a captured frame to the session's own listener, once per session.
 *
 * **Once, and that is the rule this hook exists to hold.** The capture is triggered by a media event, and
 * events that mean the same thing arrive more than once: a seek raises `seeked`, a stall and recovery can
 * raise `loadeddata` again, and React re-renders a component whose props changed. A poster is one picture
 * per title, so a second capture is a second JPEG written over the first — and the cost of getting this
 * wrong is not a wrong poster, it is a decode of the whole film per event on a machine that is trying to
 * play it.
 *
 * **The token is never handled here.** The URL comes from the core with the token already in it, and this
 * passes it to `fetch` unchanged; nothing in the renderer formats, logs, or reconstructs it.
 */

import { useCallback, useEffect, useRef, useState } from "react";

import { normalizeIpcError } from "@peregrine/ipc-client";

import { logUiError } from "../../../lib/error-log.js";
import { captureFrame } from "../lib/frame-capture.js";

/** How a capture ended, as far as anything that draws it needs to know. */
export type CaptureState = "idle" | "captured" | "refused";

/**
 * Captures one frame of whatever is playing and stores it as the title's poster.
 *
 * `thumbnailUrl` is the core's own address for the capture, and its absence is the whole condition: a
 * session whose caller had nowhere to store a poster offers no route, and this does nothing rather than
 * posting into a `404`.
 */
export function useFrameCapture(
  thumbnailUrl: string | null,
  hintMs: number | null,
): { readonly state: CaptureState; readonly capture: () => void } {
  const [state, setState] = useState<CaptureState>("idle");
  // Once per session, and a ref rather than state because a second render must not reset it.
  const attempted = useRef(false);
  // Whether this hook is still mounted: a capture takes seconds, and one that lands after the reader has
  // left the player must not set state on an unmounted component.
  const live = useRef(true);

  useEffect(() => {
    live.current = true;
    return () => {
      live.current = false;
    };
  }, []);

  // A new session is a new poster's chance: the address changing is what says "this is a different film",
  // and a title the reader starts again after a refusal deserves the retry.
  useEffect(() => {
    attempted.current = false;
    setState("idle");
  }, [thumbnailUrl]);

  const capture = useCallback((): void => {
    if (thumbnailUrl === null || attempted.current) {
      return;
    }
    attempted.current = true;
    void (async () => {
      const outcome = await captureFrame(thumbnailUrl, hintMs);
      if (!live.current) {
        return;
      }
      if (outcome.kind === "refused") {
        setState("refused");
        return;
      }
      try {
        const answer = await fetch(thumbnailUrl, {
          method: "POST",
          headers: { "Content-Type": "image/jpeg" },
          body: outcome.blob,
        });
        if (live.current) {
          setState(answer.ok ? "captured" : "refused");
        }
      } catch (caught: unknown) {
        // A refused post is a poster that will not arrive, which is a state the tile already draws. It is
        // logged rather than surfaced: there is nothing a reader can do about it, and the player is the
        // wrong place to explain a cache.
        logUiError("captureFrame", normalizeIpcError(caught));
        if (live.current) {
          setState("refused");
        }
      }
    })();
  }, [thumbnailUrl, hintMs]);

  return { state, capture };
}
