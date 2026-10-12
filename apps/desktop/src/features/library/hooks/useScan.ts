/**
 * The hook layer for scanning from the library screen.
 *
 * The core exposes two commands here and the difference matters: `scan_now` starts a walk of every
 * root, and it is refused with a conflict while one is running rather than queued, because the walk
 * is the expensive part and a queue of identical walks is not a feature. So this hook's `start`
 * reports that refusal like any other, and does not pretend to have started anything.
 *
 * Progress and the terminal phase arrive on the same two job topics Diagnostics reads, and the
 * envelope rule is shared with it (`lib/job-events.ts`) so the two cannot disagree about which
 * events are readable. Only the job this hook started is followed: another job's events are a
 * different screen's business.
 *
 * `finished` is a counter rather than a flag, and that is deliberate: a screen refreshes when it
 * *changes*, and a boolean would refresh once and then never again for the second scan.
 */

import { eventTopics, normalizeIpcError, scanLibrary } from "@peregrine/ipc-client";
import type { IpcError, JobState } from "@peregrine/ipc-client";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { useCallback, useEffect, useState } from "react";
import { logUiError } from "../../../lib/error-log.js";
import { eventPayload } from "../../../lib/job-events.js";

/** The phases that mean a scan has stopped, whichever way it stopped. */
const TERMINAL: readonly JobState[] = ["succeeded", "failed", "canceled"];

/** What the library screen needs to show and drive a scan. */
export interface ScanControl {
  /** The scan this screen started, while it is the one being followed. */
  readonly jobId: string | null;
  /** The last phase seen for it, or `null` before the first state event. */
  readonly phase: JobState | null;
  /** Completion in permille, from the job's own progress events. */
  readonly permille: number;
  /** A start is in flight. */
  readonly starting: boolean;
  /** Whether the scan this screen started has stopped. */
  readonly running: boolean;
  /** How many scans this screen has watched finish. A refresh trigger, not a status. */
  readonly finished: number;
  /** What the last start failed with, if it failed. */
  readonly error: IpcError | null;
  /** Starts a scan of every root. */
  readonly start: () => Promise<void>;
}

/** The scan control, following only the scan this screen starts. */
export function useScan(): ScanControl {
  const [state, setState] = useState<{
    jobId: string | null;
    phase: JobState | null;
    permille: number;
    starting: boolean;
    finished: number;
    error: IpcError | null;
  }>({ jobId: null, phase: null, permille: 0, starting: false, finished: 0, error: null });

  const start = useCallback(async (): Promise<void> => {
    setState((previous) => ({ ...previous, starting: true, error: null }));
    try {
      const started = await scanLibrary();
      setState((previous) => ({
        ...previous,
        starting: false,
        jobId: started.jobId,
        phase: "queued",
        permille: 0,
      }));
    } catch (caught: unknown) {
      const error = normalizeIpcError(caught);
      logUiError("scanLibrary", error);
      setState((previous) => ({ ...previous, starting: false, error }));
    }
  }, []);

  useEffect(() => {
    const unlisteners: UnlistenFn[] = [];
    let cancelled = false;

    // One handler for both topics: each carries part of one job's story, and a handler per topic
    // would need the same "is this my job" test twice.
    const subscribe = (topic: string): void => {
      void listen<unknown>(topic, (event) => {
        const payload = eventPayload(event.payload);
        if (payload === null) {
          return;
        }
        setState((previous) => {
          const jobId = payload["jobId"];
          if (previous.jobId === null || jobId !== previous.jobId) {
            return previous;
          }
          const permille =
            topic === eventTopics.jobProgress.name &&
            typeof payload["progressPermille"] === "number"
              ? Math.min(1000, Math.max(0, payload["progressPermille"]))
              : previous.permille;
          const phase = topic === eventTopics.jobState.name ? asPhase(payload["phase"]) : null;
          if (phase === null) {
            return { ...previous, permille };
          }
          const terminal = TERMINAL.includes(phase);
          return {
            ...previous,
            permille,
            phase,
            // The job is no longer followed once it has stopped: a later event about the same id
            // cannot arrive, and holding the id would make a stale one look live.
            jobId: terminal ? null : previous.jobId,
            finished: terminal ? previous.finished + 1 : previous.finished,
          };
        });
      }).then((unlisten) => {
        if (cancelled) {
          unlisten();
        } else {
          unlisteners.push(unlisten);
        }
      });
    };

    subscribe(eventTopics.jobProgress.name);
    subscribe(eventTopics.jobState.name);

    return () => {
      cancelled = true;
      for (const unlisten of unlisteners) {
        unlisten();
      }
    };
  }, []);

  const phase = state.phase;
  return {
    ...state,
    running: state.jobId !== null && (phase === null || !TERMINAL.includes(phase)),
    start,
  };
}

/** The phase the core declares, or `null` for a string this build does not know. */
function asPhase(value: unknown): JobState | null {
  const phases: readonly JobState[] = [
    "queued",
    "running",
    "deferred",
    "succeeded",
    "failed",
    "canceled",
  ];
  return typeof value === "string" && (phases as readonly string[]).includes(value)
    ? (value as JobState)
    : null;
}
