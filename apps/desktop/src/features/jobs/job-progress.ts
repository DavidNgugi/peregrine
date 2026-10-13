/**
 * The job screens' data model, as pure functions.
 *
 * The live view is a projection of two event streams and nothing else: the backend owns
 * every rule about jobs, and this module only decides which fields the screen shows. It is
 * separate from the hook that subscribes because a Node test environment has no Tauri
 * bridge, so the logic that can be wrong is tested without one.
 *
 * Everything arriving from the event stream is treated as untrusted shape (AGENTS §4,
 * "Malformed"): an event this build does not recognise is dropped rather than rendered as
 * `undefined`.
 */

import type { EventTopicName, JobRecord, JobState } from "@peregrine/ipc-client";

import { eventPayload, jobPhase } from "../../lib/job-events.js";

// The phase rule is the core's, and this module reads it rather than keeping a copy: three
// features now parse a `job://state` payload, and a list of phases that disagrees with the other
// two is a screen waiting for a stop it cannot recognise.
export { isTerminal } from "../../lib/job-events.js";

/** One live job, as the two streams describe it. */
export interface LiveJob {
  readonly jobId: string;
  readonly journalId: number;
  readonly progressPermille: number;
  readonly attempt: number;
  /** The last lifecycle phase seen; `null` until a state event arrives. */
  readonly phase: JobState | null;
}

/** A parsed event from either job stream. */
export type JobEvent =
  | { readonly kind: "progress"; readonly job: LiveJob }
  | { readonly kind: "state"; readonly job: LiveJob; readonly phase: JobState };

/** The progress a screen shows, clamped to the range the backend documents. */
export function percentOf(job: LiveJob | null): number {
  if (job === null) {
    return 0;
  }
  return Math.min(100, Math.max(0, job.progressPermille / 10));
}

function asFiniteNumber(value: unknown): number | null {
  return typeof value === "number" && Number.isFinite(value) ? value : null;
}

/**
 * Parses one event envelope into a job event, or `null` when it is not one.
 *
 * The envelope is `{ topic, schemaVersion, payload }`; the topic decides which fields the
 * payload must have. A version this build does not know is dropped, because guessing at a
 * newer shape is how a screen renders nonsense.
 */
export function parseJobEvent(topic: EventTopicName | string, envelope: unknown): JobEvent | null {
  const inner = eventPayload(envelope);
  if (inner === null) {
    return null;
  }

  const jobId = inner["jobId"];
  const journalId = asFiniteNumber(inner["journalId"]);
  const attempt = asFiniteNumber(inner["attempt"]);
  if (typeof jobId !== "string" || journalId === null || attempt === null) {
    return null;
  }

  if (topic === "job://progress") {
    const progressPermille = asFiniteNumber(inner["progressPermille"]);
    if (progressPermille === null) {
      return null;
    }
    return {
      kind: "progress",
      job: { jobId, journalId, progressPermille, attempt, phase: null },
    };
  }

  if (topic === "job://state") {
    const phase = jobPhase(inner["phase"]);
    if (phase === null) {
      return null;
    }
    return {
      kind: "state",
      job: { jobId, journalId, progressPermille: 0, attempt, phase },
      phase,
    };
  }

  return null;
}

/**
 * Folds one event into the live job.
 *
 * A state event carries no progress and a progress event carries no phase, so each keeps
 * the other's field from the previous value. An event about a different job replaces the
 * live one: the screen follows the job the user just started.
 */
export function reduceLiveJob(current: LiveJob | null, event: JobEvent): LiveJob {
  const sameJob = current !== null && current.jobId === event.job.jobId;
  const base: LiveJob = sameJob
    ? current
    : { ...event.job, progressPermille: event.job.progressPermille };

  switch (event.kind) {
    case "progress":
      return {
        ...base,
        progressPermille: Math.max(0, Math.min(1000, event.job.progressPermille)),
        attempt: event.job.attempt,
      };
    case "state":
      return { ...base, phase: event.phase, attempt: event.job.attempt };
  }
}

/** The row in the journal the live job belongs to, when it has been refreshed. */
export function findJournalRow(rows: readonly JobRecord[], job: LiveJob | null): JobRecord | null {
  if (job === null) {
    return null;
  }
  return rows.find((row) => row.id === job.journalId) ?? null;
}
