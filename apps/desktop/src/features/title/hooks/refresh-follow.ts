/**
 * The refresh a title screen asks for, as pure functions.
 *
 * Separate from the hook that subscribes for the reason `features/jobs/job-progress.ts` is separate
 * from its store: a Node test environment has no Tauri bridge, so whatever can be wrong has to be a
 * function of its inputs. Two things here can be wrong in ways nothing else would notice, and both
 * are decided here rather than inside an effect:
 *
 * * asking a provider twice for one visit, which spends a request against somebody else's API
 *   (`AGENTS R24`), and
 * * following a job that is not this screen's, or refusing to stop following one that is, which is a
 *   screen that either re-reads endlessly or never re-reads at all.
 */

import type { JobState } from "@peregrine/ipc-client";

import { eventPayload, isTerminal, jobPhase } from "../../../lib/job-events.js";

/** The refresh this screen started, and how much it has watched stop. */
export interface RefreshFollow {
  /** The job being followed, or `null` when none is. */
  readonly jobId: string | null;
  /** How many refreshes have stopped. A re-read trigger, not a status. */
  readonly finished: number;
}

/** Following nothing, which is where every visit starts. */
export const NOT_FOLLOWING: RefreshFollow = { jobId: null, finished: 0 };

/**
 * What the memory a visit keeps becomes when the route moves to `id`.
 *
 * **Leaving a title is what makes the next visit a new one**, and this is the whole of that rule: a
 * route that is not a title forgets, so coming back to the same title asks again. Without it the
 * memory would mean "once per title per app session" — a refresh that failed, or a poster that had not
 * landed before the user navigated away, could never be retried without restarting the app.
 *
 * A route that *is* a title leaves the memory alone, which is what keeps a single visit to one ask
 * however many times its read runs.
 */
export function afterRouteChange(asked: number | null, id: number | null): number | null {
  return id === null ? null : asked;
}

/**
 * Whether a read should ask a provider for this title.
 *
 * **Once per visit**, and `due` is the core's answer rather than this module's (AGENTS R7): the core
 * says yes for a title no provider has filled in and for one whose poster has been evicted, and no
 * for a title whose provider had no artwork, for a title no configured provider holds an id for, and
 * for one whose poster is already on disk. What this adds is the memory the core cannot have — the id
 * this visit has already asked about — because a refresh that produced no poster leaves the core's
 * answer unchanged, and asking again on the re-read that follows it would be a request loop.
 */
export function shouldAsk(asked: number | null, id: number, due: boolean): boolean {
  return due && asked !== id;
}

/**
 * Folds one `job://state` envelope into the follow.
 *
 * Everything that is not this job's stop leaves the follow untouched: another screen's job, an
 * envelope this build does not read, a phase it does not know, and a job that is still running are
 * all "not mine". A phase this build does not know is deliberately *not* a stop — a state machine
 * that grew a phase is not this build's to interpret, and guessing would end the wait early.
 */
export function followRefresh(follow: RefreshFollow, envelope: unknown): RefreshFollow {
  if (follow.jobId === null) {
    return follow;
  }
  const payload = eventPayload(envelope);
  if (payload === null || payload["jobId"] !== follow.jobId) {
    return follow;
  }
  const phase: JobState | null = jobPhase(payload["phase"]);
  if (phase === null || !isTerminal(phase)) {
    return follow;
  }
  // The id is released as the job stops, so a stale one cannot look live; a later event about the
  // same id cannot arrive.
  return { jobId: null, finished: follow.finished + 1 };
}
