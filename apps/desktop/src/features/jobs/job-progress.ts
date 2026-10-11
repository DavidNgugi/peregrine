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

/** The terminal phases, which are what stops the screen waiting for more. */
const TERMINAL: readonly JobState[] = ["succeeded", "failed", "canceled"];

/** Whether a phase means the job will not report again. */
export function isTerminal(phase: JobState | null): boolean {
  return phase !== null && TERMINAL.includes(phase);
}

/** The progress a screen shows, clamped to the range the backend documents. */
export function percentOf(job: LiveJob | null): number {
  if (job === null) {
    return 0;
  }
  return Math.min(100, Math.max(0, job.progressPermille / 10));
}

/** Narrows an unknown value to a record, the shape both payloads have. */
function asRecord(value: unknown): Record<string, unknown> | null {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    return null;
  }
  return value as Record<string, unknown>;
}

/** The phases the core declares, so an unknown string cannot reach the screen. */
const PHASES: readonly JobState[] = [
  "queued",
  "running",
  "deferred",
  "succeeded",
  "failed",
  "canceled",
];

function asPhase(value: unknown): JobState | null {
  return typeof value === "string" && (PHASES as readonly string[]).includes(value)
    ? (value as JobState)
    : null;
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
  const outer = asRecord(envelope);
  const inner = outer === null ? null : asRecord(outer["payload"]);
  if (outer === null || inner === null) {
    return null;
  }
  if (outer["schemaVersion"] !== 1) {
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
    const phase = asPhase(inner["phase"]);
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
