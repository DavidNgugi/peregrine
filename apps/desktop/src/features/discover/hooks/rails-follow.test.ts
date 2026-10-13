import { describe, expect, it } from "vitest";
import { NOT_FOLLOWING, followRails, railsFetchStopped } from "./rails-follow.js";

/**
 * The follow rule, as pure functions.
 *
 * The refresh itself is unobservable in a Node test — `renderToStaticMarkup` has no DOM, no event
 * loop, and no Tauri bridge — so what can be wrong is separated from the effect that listens and
 * tested here: which events stop a follow, and which are somebody else's.
 */

/** A `job://state` envelope in the shape the bus publishes. */
function envelope(payload: Record<string, unknown>, version = 1): unknown {
  return { schemaVersion: version, payload };
}

const RUNNING = envelope({ journalId: 4, jobId: "01ARZ3NDEKTSV4RRFFQ69G5FAV", phase: "running" });

describe("followRails", () => {
  it("leaves the follow alone when nothing is being followed", () => {
    expect(followRails(NOT_FOLLOWING, RUNNING)).toEqual(NOT_FOLLOWING);
  });

  it("stops on this row's terminal phase and counts it", () => {
    const following = { journalId: 4, finished: 0 };

    expect(followRails(following, envelope({ journalId: 4, phase: "succeeded" }))).toEqual({
      journalId: null,
      finished: 1,
    });
    expect(followRails(following, envelope({ journalId: 4, phase: "failed" }))).toEqual({
      journalId: null,
      finished: 1,
    });
  });

  it("keeps following a row that is still running, or deferred", () => {
    const following = { journalId: 4, finished: 0 };

    expect(followRails(following, RUNNING)).toBe(following);
    expect(followRails(following, envelope({ journalId: 4, phase: "deferred" }))).toBe(following);
  });

  it("ignores another screen's job", () => {
    // The library's scan and this screen's fetch both publish on the same topic; following the
    // wrong one would re-read the rails on somebody else's schedule.
    const following = { journalId: 4, finished: 0 };

    expect(followRails(following, envelope({ journalId: 9, phase: "succeeded" }))).toBe(following);
  });

  it("ignores an envelope this build does not read, and a phase it does not know", () => {
    // A state machine that grew a phase is not this build's to interpret, and guessing would end
    // the wait early.
    const following = { journalId: 4, finished: 0 };

    expect(followRails(following, undefined)).toBe(following);
    expect(followRails(following, { payload: { journalId: 4, phase: "succeeded" } })).toBe(
      following,
    );
    expect(followRails(following, envelope({ journalId: 4, phase: "succeeded" }, 2))).toBe(
      following,
    );
    expect(followRails(following, envelope({ journalId: 4, phase: "melted" }))).toBe(following);
    expect(followRails(following, envelope({ journalId: 4 }))).toBe(following);
  });

  it("does not match a journal id that arrives as a string", () => {
    // The payload is untrusted shape (AGENTS §4, "Malformed"): a string id is not the number the
    // command answered with, and treating it as one would follow a row this screen never started.
    const following = { journalId: 4, finished: 0 };

    expect(followRails(following, envelope({ journalId: "4", phase: "succeeded" }))).toBe(
      following,
    );
  });
});

describe("railsFetchStopped", () => {
  it("is the rails fetch reaching a terminal phase, whoever started it", () => {
    // The read starts the first fetch in the core and never learns its journal row, so its
    // completion is matched by kind rather than by id — that is what makes the screen read again and
    // draw the rails (`FLOWS F2`).
    for (const phase of ["succeeded", "failed", "canceled"]) {
      expect(railsFetchStopped(envelope({ journalId: 7, kind: "discover.rails", phase }))).toBe(
        true,
      );
    }
  });

  it("ignores a rails fetch that has not stopped, and every other job", () => {
    expect(
      railsFetchStopped(envelope({ journalId: 7, kind: "discover.rails", phase: "running" })),
    ).toBe(false);
    expect(
      railsFetchStopped(envelope({ journalId: 7, kind: "library.scan", phase: "succeeded" })),
    ).toBe(false);
    // A payload with no kind is a shape this build does not read, not a rails fetch.
    expect(railsFetchStopped(envelope({ journalId: 7, phase: "succeeded" }))).toBe(false);
    expect(railsFetchStopped(envelope({ kind: "discover.rails", phase: "melted" }))).toBe(false);
    expect(railsFetchStopped(envelope({ kind: 7, phase: "succeeded" }))).toBe(false);
    expect(railsFetchStopped(undefined)).toBe(false);
    expect(railsFetchStopped({ payload: { kind: "discover.rails", phase: "succeeded" } })).toBe(
      false,
    );
  });
});
