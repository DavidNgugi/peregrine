/**
 * Following the rails fetch, as pure functions.
 *
 * Separate from the hook for the reason `features/title/hooks/refresh-follow.ts` is separate from
 * its own: a Node test environment has no Tauri bridge, so whatever can be wrong has to be a
 * function of its inputs. Two things here can be wrong in ways nothing else would notice, and both
 * are decided here rather than inside an effect:
 *
 * * following a job that is not this screen's — another screen's refresh, or a stale journal row —
 *   which would re-read the rails on somebody else's schedule, and
 * * refusing to stop following one that has, which is a Refresh control stuck on "refreshing" for
 *   the rest of the session.
 *
 * **The journal row is what is matched, not the scheduler id.** A submission the runtime coalesced
 * onto a fetch already running has no scheduler id of its own, and every `job://state` payload
 * carries both — so the journal id is the one that is always present, always this screen's, and
 * unique while the fetch is live.
 */

import type { JobState } from "@peregrine/ipc-client";

import { eventPayload, isTerminal, jobPhase } from "../../../lib/job-events.js";

/** The fetch this screen is following, and how many it has watched stop. */
export interface RailsFollow {
  /** The journal row being followed, or `null` when none is. */
  readonly journalId: number | null;
  /** How many fetches have stopped. A re-read trigger, not a status. */
  readonly finished: number;
}

/** Following nothing, which is where every visit starts. */
export const NOT_FOLLOWING: RailsFollow = { journalId: null, finished: 0 };

/**
 * Folds one `job://state` envelope into the follow.
 *
 * Everything that is not this row's stop leaves the follow untouched: another screen's job, an
 * envelope this build does not read, a phase it does not know, and a fetch that is still running are
 * all "not mine". A phase this build does not know is deliberately *not* a stop — a state machine
 * that grew a phase is not this build's to interpret, and guessing would end the wait early.
 */
export function followRails(follow: RailsFollow, envelope: unknown): RailsFollow {
  if (follow.journalId === null) {
    return follow;
  }
  const payload = eventPayload(envelope);
  if (payload === null || payload["journalId"] !== follow.journalId) {
    return follow;
  }
  const phase: JobState | null = jobPhase(payload["phase"]);
  if (phase === null || !isTerminal(phase)) {
    return follow;
  }
  // The row is released as the fetch stops, so a stale one cannot look live; a later event about the
  // same row cannot arrive.
  return { journalId: null, finished: follow.finished + 1 };
}
