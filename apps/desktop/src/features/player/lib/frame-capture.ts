/**
 * Turning a frame of the film into the film's poster.
 *
 * **Why this is the renderer's job at all.** Decoding a video is the platform's media stack's, and the
 * surface ADR 0023 decides is a `<video>` element — so the only decoder this build has is the one behind
 * that element, and the only way to get a frame out of it is to draw it onto a canvas. There is no
 * sidecar to ask and no Rust-side decoder to call, and inventing one would be a second media stack.
 *
 * **Two halves, and the split is why the rule is testable.** [`frameSeekMs`] is arithmetic over a
 * duration and has its own tests; [`captureFrame`] is the DOM half, which this harness cannot reach at
 * all (`PG-E21-03`'s named gap). Everything that could be got *wrong* — where in a film a poster comes
 * from — is in the half that is tested.
 *
 * **The capture has its own element.** The obvious implementation seeks the element the reader is
 * watching, which means the film either starts ten per cent in or visibly jumps there and back. A
 * detached element loading the same session URL costs a few hundred kilobytes of range requests and
 * touches nothing the reader can see.
 */

/**
 * How far into a film a poster frame is taken from.
 *
 * **A tenth, with a floor and a ceiling.** The first frames of a film are the studio logos and then
 * black, which is a poster that says nothing; a tenth is far enough in that a scene is showing. The floor
 * keeps a very short clip from asking for a frame past its own end, and the ceiling keeps a very long one
 * from spending ten minutes of a feature — which for a two-hour file would be past the opening titles and
 * into the first act, but for a ten-hour serial would be an hour in, and a poster is not worth a seek that
 * far.
 *
 * Exported because it is a rule rather than a constant: a screen that decided this for itself would be a
 * second answer to "where does a poster come from".
 */
export const FRAME_SEEK_MS = 30_000;

/** The fraction of a film a poster is taken from, when it is longer than the floor. */
const FRAME_FRACTION = 0.1;

/**
 * Where to seek for a poster frame, given how long the film is.
 *
 * Answers `0` for a duration this build cannot use — zero, negative, `NaN`, or not finite — because a
 * seek to a nonsense time is a seek that never completes, and the element's first frame is a better answer
 * than a capture that hangs.
 */
export function frameSeekMs(durationMs: number): number {
  if (!Number.isFinite(durationMs) || durationMs <= 0) {
    return 0;
  }
  // Never past the end: `seeked` does not fire for a time beyond the duration on some engines, so a
  // capture that asked for one would wait for an event that never comes.
  const endurable = Math.max(durationMs - 1_000, 0);
  return Math.min(Math.max(durationMs * FRAME_FRACTION, FRAME_SEEK_MS), endurable);
}

/**
 * The largest edge a captured frame is scaled down to.
 *
 * A poster is drawn in a 342-wide box on a tile and no larger, so a 1080p frame stored at its native size
 * would be four megabytes of cache for a picture displayed at a sixth of it. 640 keeps a frame sharp on a
 * 2× display and small enough that a whole library's worth of them is not a cache problem.
 */
const MAX_FRAME_EDGE = 640;

/** What a capture produced, or why it produced nothing. */
export type CaptureOutcome =
  | { readonly kind: "captured"; readonly blob: Blob }
  | { readonly kind: "refused" };

/**
 * Draws one frame of `url` onto a canvas and answers it as a JPEG.
 *
 * Every failure is `refused` rather than a thrown error: a poster that could not be made is a state the
 * caller already draws — the reserved box — and there is nothing a reader can do about a decoder that
 * would not seek.
 *
 * `hintMs` is the playing element's duration when there is one. It is a *hint*: the element this function
 * creates has read the same header by the time it is used, so a caller with nothing to pass — the poster
 * backfill, which has no player on screen — passes `null` and gets the same answer.
 */
export async function captureFrame(
  url: string,
  hintMs: number | null,
): Promise<CaptureOutcome> {
  const video = document.createElement("video");
  // The response has to be CORS-readable or the canvas is tainted and `toBlob` refuses; the session's
  // listener sends `Access-Control-Allow-Origin` for exactly this reason.
  video.crossOrigin = "anonymous";
  video.muted = true;
  video.playsInline = true;
  video.preload = "auto";
  // Off-screen rather than `display: none`, because some engines will not decode a video that is not
  // laid out, and 1 px rather than 0 because a zero-sized element is the same hazard in some of them.
  video.style.cssText =
    "position:fixed;left:-10000px;top:0;width:1px;height:1px;opacity:0;pointer-events:none";
  video.src = url;

  try {
    await once(video, "loadeddata");
    // **The element's own duration, and the hint only when it is a number.** A backfill has no playing
    // element to read a duration from, and the element here has read the header already — so asking it is
    // both simpler and more accurate than plumbing a duration through every caller.
    const durationMs = hintMs !== null && hintMs > 0 ? hintMs : video.duration * 1000;
    video.currentTime = frameSeekMs(durationMs) / 1000;
    // `seeked` is not guaranteed to fire for a time the element is already at, so a seek to zero resolves
    // on the next task rather than waiting for an event that will not come.
    if (video.currentTime > 0) {
      await once(video, "seeked");
    }

    const width = video.videoWidth;
    const height = video.videoHeight;
    if (width === 0 || height === 0) {
      return { kind: "refused" };
    }
    const scale = Math.min(1, MAX_FRAME_EDGE / Math.max(width, height));
    const canvas = document.createElement("canvas");
    canvas.width = Math.max(1, Math.round(width * scale));
    canvas.height = Math.max(1, Math.round(height * scale));
    const context = canvas.getContext("2d");
    if (context === null) {
      return { kind: "refused" };
    }
    context.drawImage(video, 0, 0, canvas.width, canvas.height);

    const blob = await toJpeg(canvas);
    return blob === null ? { kind: "refused" } : { kind: "captured", blob };
  } catch {
    // A decode failure, a tainted canvas, a seek that never completed: one answer, because the caller's
    // response to all of them is the same reserved box.
    return { kind: "refused" };
  } finally {
    // The element holds a stream; dropping the source is what releases it rather than leaving a second
    // connection open for the life of the session.
    video.removeAttribute("src");
    video.load();
  }
}

/** Waits for one media event, or refuses if it never arrives. */
function once(element: HTMLVideoElement, event: string): Promise<void> {
  return new Promise((resolve, reject) => {
    const timer = window.setTimeout(() => {
      reject(new Error(`${event} did not arrive`));
    }, CAPTURE_TIMEOUT_MS);
    const done = (): void => {
      window.clearTimeout(timer);
      element.removeEventListener(event, done);
      element.removeEventListener("error", failed);
      resolve();
    };
    const failed = (): void => {
      window.clearTimeout(timer);
      element.removeEventListener(event, done);
      element.removeEventListener("error", failed);
      reject(new Error("the frame could not be decoded"));
    };
    element.addEventListener(event, done, { once: true });
    element.addEventListener("error", failed, { once: true });
  });
}

/**
 * How long a capture waits for the element before giving up.
 *
 * A file on a local disk answers in milliseconds; this is the bound that stops a stall — a drive that went
 * to sleep mid-read, a codec the platform is fighting — from leaving a promise pending forever, which
 * would be a poster that never arrives and never fails either.
 */
const CAPTURE_TIMEOUT_MS = 15_000;

/** A canvas as a JPEG, or `null` when it will not encode one. */
function toJpeg(canvas: HTMLCanvasElement): Promise<Blob | null> {
  return new Promise((resolve) => {
    try {
      canvas.toBlob(
        (blob) => {
          resolve(blob);
        },
        "image/jpeg",
        0.8,
      );
    } catch {
      // A tainted canvas throws here rather than calling back, which is the state a missing
      // `Access-Control-Allow-Origin` produces.
      resolve(null);
    }
  });
}
