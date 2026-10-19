import { describe, expect, it } from "vitest";

import { providerFillFinished } from "./useLibraryTitles.js";

/**
 * The re-read trigger, as a pure function.
 *
 * The listener itself cannot be rendered in a Node test — `renderToStaticMarkup` has no event loop
 * and no Tauri bridge — so what can be wrong is separated from the effect that listens: which
 * `job://state` payloads mean a provider fill has finished.
 */

/** A `job://state` envelope in the shape the bus publishes. */
function envelope(payload: unknown, version = 1): unknown {
  return { topic: "job://state", schemaVersion: version, payload };
}

/** A job payload, with the fields this rule does not read filled in. */
function job(overrides: Record<string, unknown> = {}, version = 1): unknown {
  return envelope(
    {
      jobId: "01ARZ3NDEKTSV4RRFFQ69G5FAV",
      journalId: 4,
      kind: "provider.refresh_title",
      phase: "succeeded",
      attempt: 1,
      ...overrides,
    },
    version,
  );
}

describe("providerFillFinished", () => {
  it("is true for a provider fill that succeeded", () => {
    expect(providerFillFinished(job())).toBe(true);
  });

  it("is false while the fill is still running", () => {
    expect(providerFillFinished(job({ phase: "running" }))).toBe(false);
  });

  it("is false for a fill that failed", () => {
    expect(providerFillFinished(job({ phase: "failed" }))).toBe(false);
  });

  it("is false for another job kind that succeeded", () => {
    expect(providerFillFinished(job({ kind: "library.scan" }))).toBe(false);
  });

  it("is false for a payload missing the fields the rule reads", () => {
    expect(providerFillFinished(envelope({ journalId: 4 }))).toBe(false);
  });

  it("is false for anything that is not a readable envelope", () => {
    expect(providerFillFinished(null)).toBe(false);
    expect(providerFillFinished([job()])).toBe(false);
    expect(providerFillFinished(job({}, 2))).toBe(false);
    expect(providerFillFinished({ schemaVersion: 1, payload: "succeeded" })).toBe(false);
  });
});
