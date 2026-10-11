import { describe, expect, it } from "vitest";
import {
  findJournalRow,
  isTerminal,
  parseJobEvent,
  percentOf,
  reduceLiveJob,
  type LiveJob,
} from "./job-progress.js";

const job = "01ARZ3NDEKTSV4RRFFQ69G5FAV";

function envelope(topic: string, payload: unknown, version = 1): unknown {
  return { topic, schemaVersion: version, payload };
}

describe("parseJobEvent", () => {
  it("reads a progress payload", () => {
    const parsed = parseJobEvent(
      "job://progress",
      envelope("job://progress", {
        jobId: job,
        journalId: 3,
        progressPermille: 250,
        attempt: 1,
      }),
    );
    expect(parsed).toEqual({
      kind: "progress",
      job: { jobId: job, journalId: 3, progressPermille: 250, attempt: 1, phase: null },
    });
  });

  it("reads a state payload", () => {
    const parsed = parseJobEvent(
      "job://state",
      envelope("job://state", {
        jobId: job,
        journalId: 3,
        kind: "hello",
        phase: "canceled",
        attempt: 2,
      }),
    );
    expect(parsed).toMatchObject({ kind: "state", phase: "canceled" });
  });

  it("drops an envelope it does not recognise rather than rendering it", () => {
    const malformed: unknown[] = [
      null,
      undefined,
      42,
      "a string",
      [],
      {},
      { topic: "job://progress" },
      { schemaVersion: 1, payload: { jobId: job, journalId: 1, attempt: 1 } },
      // A payload that is not an object.
      envelope("job://progress", "nope"),
      // A missing field.
      envelope("job://progress", { jobId: job, journalId: 1, attempt: 1 }),
      // A field of the wrong type.
      envelope("job://progress", {
        jobId: 7,
        journalId: 1,
        progressPermille: 1,
        attempt: 1,
      }),
      // A number that is not finite.
      envelope("job://progress", {
        jobId: job,
        journalId: 1,
        progressPermille: Number.NaN,
        attempt: 1,
      }),
      // A phase this build does not declare.
      envelope("job://state", {
        jobId: job,
        journalId: 1,
        phase: "exploded",
        attempt: 1,
      }),
      // A topic that is not one of ours.
      envelope("library://changed", { jobId: job, journalId: 1 }),
      // A schema version from the future.
      envelope("job://progress", { jobId: job, journalId: 1, progressPermille: 1, attempt: 1 }, 2),
    ];

    for (const raw of malformed) {
      expect(parseJobEvent("job://progress", raw), JSON.stringify(raw)).toBeNull();
    }
  });
});

describe("reduceLiveJob", () => {
  it("keeps the phase a progress event does not carry", () => {
    const running = reduceLiveJob(null, {
      kind: "state",
      job: { jobId: job, journalId: 1, progressPermille: 0, attempt: 1, phase: "running" },
      phase: "running",
    });
    const progressed = reduceLiveJob(running, {
      kind: "progress",
      job: { jobId: job, journalId: 1, progressPermille: 400, attempt: 1, phase: null },
    });

    expect(progressed.phase).toBe("running");
    expect(progressed.progressPermille).toBe(400);
  });

  it("keeps the progress a state event does not carry", () => {
    const started = reduceLiveJob(null, {
      kind: "progress",
      job: { jobId: job, journalId: 9, progressPermille: 700, attempt: 1, phase: null },
    });
    const finished = reduceLiveJob(started, {
      kind: "state",
      job: { jobId: job, journalId: 9, progressPermille: 0, attempt: 1, phase: "succeeded" },
      phase: "succeeded",
    });

    expect(finished.phase).toBe("succeeded");
    expect(finished.progressPermille).toBe(700);
  });

  it("follows a different job", () => {
    const first: LiveJob = {
      jobId: job,
      journalId: 1,
      progressPermille: 900,
      attempt: 1,
      phase: "running",
    };
    const other = reduceLiveJob(first, {
      kind: "state",
      job: {
        jobId: "01ARZ3NDEKTSV4RRFFQ69G5FBW",
        journalId: 2,
        progressPermille: 0,
        attempt: 1,
        phase: "queued",
      },
      phase: "queued",
    });

    expect(other.jobId).toBe("01ARZ3NDEKTSV4RRFFQ69G5FBW");
    expect(other.progressPermille).toBe(0);
  });

  it("clamps a value outside the documented range", () => {
    const tooBig = reduceLiveJob(null, {
      kind: "progress",
      job: { jobId: job, journalId: 1, progressPermille: 5_000, attempt: 1, phase: null },
    });
    expect(tooBig.progressPermille).toBe(1000);
    expect(percentOf(tooBig)).toBe(100);
  });
});

describe("isTerminal", () => {
  it("is true only for a phase that will not report again", () => {
    expect(isTerminal(null)).toBe(false);
    expect(isTerminal("queued")).toBe(false);
    expect(isTerminal("running")).toBe(false);
    expect(isTerminal("deferred")).toBe(false);
    expect(isTerminal("succeeded")).toBe(true);
    expect(isTerminal("failed")).toBe(true);
    expect(isTerminal("canceled")).toBe(true);
  });
});

describe("findJournalRow", () => {
  const rows = [
    { id: 2, kind: "hello", progressPermille: 1000 } as never,
    { id: 1, kind: "hello", progressPermille: 0 } as never,
  ];

  it("finds the row by journal id", () => {
    const live: LiveJob = {
      jobId: job,
      journalId: 1,
      progressPermille: 0,
      attempt: 1,
      phase: "queued",
    };
    expect(findJournalRow(rows, live)?.id).toBe(1);
  });

  it("answers with nothing when there is no live job or no row", () => {
    expect(findJournalRow(rows, null)).toBeNull();
    expect(
      findJournalRow(rows, {
        jobId: job,
        journalId: 99,
        progressPermille: 0,
        attempt: 1,
        phase: null,
      }),
    ).toBeNull();
  });
});
