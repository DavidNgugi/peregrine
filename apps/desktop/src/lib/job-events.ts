/**
 * The envelope rule for the core's event topics, in one place.
 *
 * Every topic the backend publishes arrives as `{ schemaVersion, payload }`, and every consumer
 * treats what is inside as untrusted shape (AGENTS §4, "Malformed"): a version this build does not
 * read, a payload that is not an object, or a field of the wrong type is dropped rather than
 * rendered as `undefined`. Two features need that rule — Diagnostics reads whole job events, and
 * the library watches one scan job's phase — and a rule with two implementations is a rule that
 * will be fixed once.
 *
 * The payload is returned as a plain record rather than as a typed event because what each caller
 * needs from it differs: the jobs bridge projects a live job, and the library reads three fields.
 * What must not differ is which envelopes count as readable.
 */

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
