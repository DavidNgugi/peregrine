/**
 * The job diagnostics screen's data layer.
 *
 * A vanilla Zustand store rather than component state, for one reason: the event
 * subscription has to outlive the screen. A user who starts a job and switches to Discover
 * must still be able to cancel it when they come back, and a `useState` inside the screen
 * would have dropped the job's identity on unmount. `useJobsBridge` owns the subscription
 * and the shell mounts it once.
 *
 * The store never computes a rule. `isTerminal` and the progress clamp are presentation
 * decisions about what to show; every decision about the job itself is in the core.
 */

import {
  commands,
  normalizeIpcError,
  unwrap,
  type Health,
  type IpcError,
  type JobRecord,
  type JobState,
} from "@peregrine/ipc-client";
import { createStore } from "zustand/vanilla";

import { logUiError } from "../../lib/error-log.js";
import { isTerminal, reduceLiveJob, type JobEvent, type LiveJob } from "./job-progress.js";

/** How many journal rows and log lines one refresh asks for. */
export const JOURNAL_PAGE = 20;
export const LOG_LINES = 100;

export interface JobsState {
  readonly live: LiveJob | null;
  readonly journal: readonly JobRecord[];
  readonly health: Health | null;
  readonly logLines: readonly string[];
  readonly error: IpcError | null;
  readonly starting: boolean;
  readonly cancelling: boolean;
  readonly refreshing: boolean;
  readonly start: (steps: number) => Promise<void>;
  readonly cancel: () => Promise<void>;
  readonly refresh: () => Promise<void>;
  readonly apply: (event: JobEvent) => void;
  readonly clearError: () => void;
}

/** Reports a failure once, to the screen and to the structured UI log. */
function report(context: string, error: unknown, set: (partial: Partial<JobsState>) => void): void {
  const normalized = normalizeIpcError(error);
  logUiError(context, normalized);
  set({ error: normalized });
}

export const jobsStore = createStore<JobsState>((set, get) => ({
  live: null,
  journal: [],
  health: null,
  logLines: [],
  error: null,
  starting: false,
  cancelling: false,
  refreshing: false,

  clearError: () => {
    set({ error: null });
  },

  start: async (steps) => {
    set({ starting: true, error: null });
    try {
      const started = await unwrap(commands.startHello(steps));
      set({
        live: {
          jobId: started.jobId,
          journalId: started.journalId,
          progressPermille: 0,
          attempt: 0,
          phase: "queued" satisfies JobState,
        },
      });
      await get().refresh();
    } catch (error) {
      report("jobs.start", error, set);
    } finally {
      set({ starting: false });
    }
  },

  cancel: async () => {
    const live = get().live;
    if (live === null) {
      return;
    }
    set({ cancelling: true, error: null });
    try {
      await unwrap(commands.cancel(live.jobId));
      // The phase that says `canceled` arrives as an event; refreshing here would race it.
    } catch (error) {
      report("jobs.cancel", error, set);
    } finally {
      set({ cancelling: false });
    }
  },

  refresh: async () => {
    set({ refreshing: true });
    try {
      const [journal, health, logLines] = await Promise.all([
        unwrap(commands.list(JOURNAL_PAGE)),
        unwrap(commands.health()),
        unwrap(commands.logTail(LOG_LINES)),
      ]);
      set({ journal, health, logLines, error: null });
    } catch (error) {
      report("jobs.refresh", error, set);
    } finally {
      set({ refreshing: false });
    }
  },

  apply: (event) => {
    const live = reduceLiveJob(get().live, event);
    set({ live });
    if (event.kind === "state" && isTerminal(event.phase)) {
      // The journal is the record: a terminal phase is the moment to read the row the
      // core wrote rather than to guess its final values from the event stream.
      void get().refresh();
    }
  },
}));
