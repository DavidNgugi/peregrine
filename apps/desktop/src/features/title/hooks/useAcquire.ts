/**
 * The title page's acquisition control: ask the core to find and queue a release, then follow it.
 *
 * `acquire` starts a job and answers its id, because the work leaves the machine — search, metadata,
 * the gate — and a command that waited would be a frozen page (R10). The job's own two topics are
 * then followed here, so the page can say what is happening and how it ended; the queue itself is
 * still the Downloads screen's to draw.
 */

import { acquire, commands, eventTopics, normalizeIpcError, unwrap } from "@peregrine/ipc-client";
import type { IpcError, JobState } from "@peregrine/ipc-client";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { useCallback, useEffect, useState } from "react";
import { logUiError } from "../../../lib/error-log.js";
import { eventPayload, isTerminal, jobPhase } from "../../../lib/job-events.js";

/** How the last acquire ended, or `null` while it has not. */
export type AcquireOutcome = "queued" | "failed" | null;

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
  /** The core's own sentence for a failure, when the journal carries one. */
  readonly failure: string | null;
  /** What the start itself refused with, or `null`. */
  readonly error: IpcError | null;
  /** The title this state belongs to, or `null` before the first start. A control held by the shell
   * would otherwise show one title's job on every other title's page. */
  readonly titleId: number | null;
  /** Asks the core to acquire the title. */
  readonly start: (titleId: number) => Promise<void>;
}

interface AcquireState {
  titleId: number | null;
  jobId: string | null;
  journalId: number | null;
  phase: JobState | null;
  permille: number;
  starting: boolean;
  outcome: AcquireOutcome;
  failure: string | null;
  error: IpcError | null;
}

const IDLE: AcquireState = {
  titleId: null,
  jobId: null,
  journalId: null,
  phase: null,
  permille: 0,
  starting: false,
  outcome: null,
  failure: null,
  error: null,
};

/** The acquisition control for one title. */
export function useAcquire(): AcquireControl {
  const [state, setState] = useState<AcquireState>(IDLE);

  const start = useCallback(async (titleId: number): Promise<void> => {
    setState({ ...IDLE, titleId, starting: true });
    try {
      const started = await acquire(titleId);
      setState({
        ...IDLE,
        titleId,
        jobId: started.jobId,
        journalId: started.journalId,
        phase: "queued",
      });
    } catch (caught: unknown) {
      const error = normalizeIpcError(caught);
      logUiError("acquire", error);
      setState({ ...IDLE, titleId, error });
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
            outcome: phase === "succeeded" ? "queued" : "failed",
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

  return { ...state, running: state.jobId !== null && !isTerminal(state.phase), start };
}
