/**
 * The player's half of Continue Watching: seek to the stored position once the media is ready, and
 * report the position as it advances.
 *
 * **Throttled by media time, not by the wall clock.** A file fires `timeupdate` four or five times a
 * second and every report is a write and a bus publish; five seconds of *media* is finer than a viewer
 * can tell, coarse enough that a two-hour film is a few hundred writes rather than ten thousand, and
 * it needs no clock the core cannot see. The last position is also reported on pause and on unmount,
 * because the moment a viewer leaves is the moment the rail most needs to be right.
 *
 * **The seek happens once, on the first `loadedmetadata`.** A media element fires that event again
 * after a seek, and re-seeking to the stored position every time would trap the viewer at the spot
 * they were trying to leave.
 */

import { normalizeIpcError, recordProgress } from "@peregrine/ipc-client";
import type { PlaybackSessionDto } from "@peregrine/ipc-client";
import type { RefObject } from "react";
import { useEffect, useRef } from "react";

import { logUiError } from "../../../lib/error-log.js";

/** How much media must pass between two reports. */
const REPORT_EVERY_SECONDS = 5;

export function usePlaybackProgress(
  surface: RefObject<HTMLVideoElement | null>,
  session: PlaybackSessionDto | null,
): void {
  // Whether the resume seek has happened for this session.
  const seeked = useRef(false);
  // The media position the last report was taken at.
  const reportedAt = useRef(0);

  useEffect(() => {
    seeked.current = false;
    reportedAt.current = 0;
  }, [session?.url]);

  useEffect(() => {
    const element = surface.current;
    if (element === null || session === null) {
      return;
    }
    const titleId = session.titleId;
    const report = (positionSeconds: number, durationSeconds: number): void => {
      void recordProgress({
        titleId,
        episodeId: null,
        positionMs: Math.max(0, Math.round(positionSeconds * 1000)),
        durationMs: Number.isFinite(durationSeconds) ? Math.round(durationSeconds * 1000) : null,
      }).catch((caught: unknown) => {
        logUiError("recordProgress", normalizeIpcError(caught));
      });
    };

    const onLoadedMetadata = (): void => {
      const resume = session.resumePositionMs;
      if (!seeked.current && resume !== null && resume > 0) {
        seeked.current = true;
        element.currentTime = resume / 1000;
      }
    };
    const onTimeUpdate = (): void => {
      if (Math.abs(element.currentTime - reportedAt.current) < REPORT_EVERY_SECONDS) {
        return;
      }
      reportedAt.current = element.currentTime;
      report(element.currentTime, element.duration);
    };
    const onPause = (): void => {
      report(element.currentTime, element.duration);
    };

    element.addEventListener("loadedmetadata", onLoadedMetadata);
    element.addEventListener("timeupdate", onTimeUpdate);
    element.addEventListener("pause", onPause);
    return () => {
      element.removeEventListener("loadedmetadata", onLoadedMetadata);
      element.removeEventListener("timeupdate", onTimeUpdate);
      element.removeEventListener("pause", onPause);
      report(element.currentTime, element.duration);
    };
  }, [surface, session]);
}
