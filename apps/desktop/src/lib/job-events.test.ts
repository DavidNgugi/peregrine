import { describe, expect, it } from "vitest";
import { eventPayload, isTerminal, jobPhase } from "./job-events.js";

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

describe("jobPhase", () => {
  it("names the phases the core declares", () => {
    for (const phase of ["queued", "running", "deferred", "succeeded", "failed", "canceled"]) {
      expect(jobPhase(phase)).toBe(phase);
    }
  });

  it("refuses a string this build does not know, which is not a phase", () => {
    // A phase from a newer state machine is not this build's to interpret: treating one as live
    // would leave a screen waiting for a stop it cannot recognise.
    for (const unknown of ["paused", "Succeeded", "", " running", null, 7, {}, ["running"]]) {
      expect(jobPhase(unknown)).toBeNull();
    }
  });
});

describe("isTerminal", () => {
  it("is true only for the phases a job does not leave", () => {
    expect(isTerminal("succeeded")).toBe(true);
    expect(isTerminal("failed")).toBe(true);
    expect(isTerminal("canceled")).toBe(true);
    expect(isTerminal("queued")).toBe(false);
    expect(isTerminal("running")).toBe(false);
    expect(isTerminal("deferred")).toBe(false);
    expect(isTerminal(null)).toBe(false);
  });
});
