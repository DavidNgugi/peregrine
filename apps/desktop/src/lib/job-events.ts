/**
 * The envelope rule for the core's event topics, in one place.
 *
 * Every topic the backend publishes arrives as `{ schemaVersion, payload }`, and every consumer
 * treats what is inside as untrusted shape (AGENTS §4, "Malformed"): a version this build does not
 * read, a payload that is not an object, or a field of the wrong type is dropped rather than
 * rendered as `undefined`. Three features need that rule — Diagnostics reads whole job events, the
 * library watches one scan job's phase, and the title screen watches one refresh — and a rule with
 * two implementations is a rule that will be fixed once.
 *
 * The payload is returned as a plain record rather than as a typed event because what each caller
 * needs from it differs: the jobs bridge projects a live job, and the library and title screens read
 * three fields. What must not differ is which envelopes count as readable — and which strings count
 * as a job phase, because a phase this build does not know is not a phase, and a screen that treated
 * one as live would wait for a stop it cannot recognise.
 */

import type { JobState } from "@peregrine/ipc-client";

/** The inner payload of a versioned envelope, or `null` when this build does not read it. */
export function eventPayload(
  envelope: unknown,
  expectedVersion = 1,
): Record<string, unknown> | null {
  const outer = asRecord(envelope);
  if (outer === null || outer["schemaVersion"] !== expectedVersion) {
    return null;
  }
  return asRecord(outer["payload"]);
}

/** A value that is an object rather than an array or `null`. */
function asRecord(value: unknown): Record<string, unknown> | null {
  return typeof value === "object" && value !== null && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : null;
}

/** The phases the core's job state machine declares, in the order it declares them. */
const PHASES: readonly JobState[] = [
  "queued",
  "running",
  "deferred",
  "succeeded",
  "failed",
  "canceled",
];

/** The phases a job does not leave, so a job in one of them will not report again. */
const TERMINAL: readonly JobState[] = ["succeeded", "failed", "canceled"];

/**
 * The phase a `job://state` payload names, or `null` for a string this build does not know.
 *
 * A phase is only a phase if the core declared it: guessing at a newer state machine is how a screen
 * ends up waiting forever for a job that has already stopped.
 */
export function jobPhase(value: unknown): JobState | null {
  return typeof value === "string" && (PHASES as readonly string[]).includes(value)
    ? (value as JobState)
    : null;
}

/** Whether a phase means the job will not report again. */
export function isTerminal(phase: JobState | null): boolean {
  return phase !== null && TERMINAL.includes(phase);
}
