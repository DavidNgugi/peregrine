import { describe, expect, it } from "vitest";
import { eventPayload } from "./job-events.js";

describe("eventPayload", () => {
  it("returns the payload of an envelope this build reads", () => {
    expect(eventPayload({ schemaVersion: 1, payload: { jobId: "abc" } })).toEqual({ jobId: "abc" });
  });

  it("refuses a version it does not read rather than guessing at the shape", () => {
    // A newer core's payload is not this build's to interpret: rendering it would show fields that
    // mean something else, and dropping it shows nothing, which is the honest answer.
    expect(eventPayload({ schemaVersion: 2, payload: { jobId: "abc" } })).toBeNull();
    expect(eventPayload({ payload: { jobId: "abc" } })).toBeNull();
  });

  it("refuses everything that is not an envelope with an object inside", () => {
    for (const malformed of [
      null,
      undefined,
      "job://state",
      42,
      [],
      { schemaVersion: 1 },
      { schemaVersion: 1, payload: null },
      { schemaVersion: 1, payload: [1, 2] },
      { schemaVersion: 1, payload: "not an object" },
    ]) {
      expect(eventPayload(malformed)).toBeNull();
    }
  });
});
