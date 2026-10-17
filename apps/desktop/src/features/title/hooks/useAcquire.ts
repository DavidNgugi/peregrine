/**
 * The title page's acquisition control: ask the core to find and queue a release, then follow it.
 *
 * `acquire` starts a job and answers its id, because the work leaves the machine — search, metadata,
 * the gate — and a command that waited would be a frozen page (R10). The job's own two topics are
 * then followed here, so the page can say what is happening and how it ended; the queue itself is
 * still the Downloads screen's to draw.
 */

import {
  commands,
  eventTopics,
  listDownloads,
  normalizeIpcError,
  unwrap,
} from "@peregrine/ipc-client";
import type { IpcError, JobState, SafetyFindingDto } from "@peregrine/ipc-client";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { useCallback, useEffect, useState } from "react";
import { logUiError } from "../../../lib/error-log.js";
import { eventPayload, isTerminal, jobPhase } from "../../../lib/job-events.js";

/** How the last acquire ended, or `null` while it has not. */
export type AcquireOutcome = "queued" | "blocked" | "failed" | null;

/** One episode a page asked to acquire, which narrows the search from the title to that episode. */
export interface EpisodePick {
  /** The season the episode belongs to. */
  readonly season: number;
  /** The episode's number within that season. */
  readonly episode: number;
}

/** What the title page's Download control reads. */
export interface AcquireControl {
  /** The request is in flight, before the core has answered with a job. */
  readonly starting: boolean;
  /** A job this page started is still running. */
  readonly running: boolean;
  /** The last phase the core reported, or `null` before the first state event. */
  readonly phase: JobState | null;
  /** Completion in permille, from the job's progress events. */
  readonly permille: number;
  /** How the last acquire ended. */
  readonly outcome: AcquireOutcome;
  /** The safety gate's findings when `outcome` is `blocked`; empty otherwise. */
  readonly findings: readonly SafetyFindingDto[];
  /** The core's own sentence for a failure, when the journal carries one. */
  readonly failure: string | null;
  /** What the start itself refused with, or `null`. */
  readonly error: IpcError | null;
  /** The title this state belongs to, or `null` before the first start. A control held by the shell
   * would otherwise show one title's job on every other title's page. */
  readonly titleId: number | null;
  /** The episode the last start named, or `null` when it asked for the whole title. */
  readonly episode: EpisodePick | null;
  /** Asks the core to acquire the title, or one episode of it when one is given. */
  readonly start: (titleId: number, episode?: EpisodePick) => Promise<void>;
}

interface AcquireState {
  titleId: number | null;
  episode: EpisodePick | null;
  jobId: string | null;
  journalId: number | null;
  phase: JobState | null;
  permille: number;
  starting: boolean;
  outcome: AcquireOutcome;
  findings: SafetyFindingDto[];
  /** A succeeded job whose created row has not been read back yet. */
  settled: boolean;
  failure: string | null;
  error: IpcError | null;
}

const IDLE: AcquireState = {
  titleId: null,
  episode: null,
  jobId: null,
  journalId: null,
  phase: null,
  permille: 0,
  starting: false,
  outcome: null,
  findings: [],
  settled: false,
  failure: null,
  error: null,
};

/** The acquisition control for one title. */
export function useAcquire(): AcquireControl {
  const [state, setState] = useState<AcquireState>(IDLE);

  const start = useCallback(async (titleId: number, episode?: EpisodePick): Promise<void> => {
    setState({ ...IDLE, titleId, episode: episode ?? null, starting: true });
    try {
      const started = await unwrap(
        commands.acquire({ id: titleId }, null, episode?.season ?? null, episode?.episode ?? null),
      );
      setState({
        ...IDLE,
        titleId,
        episode: episode ?? null,
        jobId: started.jobId,
        journalId: started.journalId,
        phase: "queued",
      });
    } catch (caught: unknown) {
      const error = normalizeIpcError(caught);
      logUiError("acquire", error);
      setState({ ...IDLE, titleId, episode: episode ?? null, error });
    }
  }, []);

  useEffect(() => {
    const unlisteners: UnlistenFn[] = [];
    let cancelled = false;

    // One handler for both topics, and only the job this page started: another title's job, or a
    // second visit's, is not this control's story.
    const subscribe = (topic: string): void => {
      void listen<unknown>(topic, (event) => {
        const payload = eventPayload(event.payload);
        if (payload === null) {
          return;
        }
        setState((previous) => {
          if (previous.jobId === null || payload["jobId"] !== previous.jobId) {
            return previous;
          }
          const permille =
            topic === eventTopics.jobProgress.name &&
            typeof payload["progressPermille"] === "number"
              ? Math.min(1000, Math.max(0, payload["progressPermille"]))
              : previous.permille;
          const phase = topic === eventTopics.jobState.name ? jobPhase(payload["phase"]) : null;
          if (phase === null) {
            return { ...previous, permille };
          }
          if (!isTerminal(phase)) {
            return { ...previous, permille, phase };
          }
          return {
            ...previous,
            permille,
            phase,
            jobId: null,
            // Success is not the end of the story: the safety gate may have parked the row it
            // created as `blocked`, so the row is read back before an outcome is claimed.
            outcome: phase === "succeeded" ? null : "failed",
            settled: phase === "succeeded",
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

  // A failed job's sentence lives on its journal row, not in the event, so it is read back once the
  // phase says the job stopped. A read that fails leaves the outcome alone: the failure is still a
  // failure, it just has no sentence to show.
  const { outcome, journalId } = state;
  useEffect(() => {
    if (outcome !== "failed" || journalId === null) {
      return;
    }
    let cancelled = false;
    void (async () => {
      try {
        const rows = await unwrap(commands.list(50));
        const row = rows.find((candidate) => candidate.id === journalId);
        const failure = row?.lastError ?? null;
        if (!cancelled && failure !== null) {
          setState((previous) => ({ ...previous, failure }));
        }
      } catch (caught: unknown) {
        logUiError("acquire.journal", normalizeIpcError(caught));
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [outcome, journalId]);

  // A job that succeeded has still not said what happened: the gate may have parked the created row
  // as blocked, in which case a download was never started. The row's own persisted report is the
  // answer, so it is read rather than recomputed (invariant I4).
  const { settled, titleId } = state;
  useEffect(() => {
    if (!settled || titleId === null) {
      return;
    }
    let cancelled = false;
    void (async () => {
      try {
        const rows = await listDownloads();
        const row = rows.find((candidate) => candidate.titleId === titleId);
        if (cancelled) {
          return;
        }
        if (row?.state === "blocked") {
          setState((previous) => ({
            ...previous,
            settled: false,
            outcome: "blocked",
            findings: row.findings,
          }));
        } else {
          setState((previous) => ({ ...previous, settled: false, outcome: "queued" }));
        }
      } catch (caught: unknown) {
        logUiError("acquire.downloads", normalizeIpcError(caught));
        if (!cancelled) {
          setState((previous) => ({ ...previous, settled: false, outcome: "queued" }));
        }
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [settled, titleId]);

  return { ...state, running: state.jobId !== null && !isTerminal(state.phase), start };
}
