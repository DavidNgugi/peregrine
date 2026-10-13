import { describe, expect, it } from "vitest";

import {
  afterRouteChange,
  followRefresh,
  NOT_FOLLOWING,
  shouldAsk,
  type RefreshFollow,
} from "./refresh-follow.js";

/** The envelope the core publishes for one job's state. */
function state(jobId: string, phase: string): unknown {
  return {
    schemaVersion: 1,
    payload: { jobId, journalId: 1, kind: "provider.refresh_title", phase },
  };
}

/** A follow of `jobId` that has already watched `finished` refreshes stop. */
function following(jobId: string, finished = 0): RefreshFollow {
  return { jobId, finished };
}

describe("afterRouteChange", () => {
  it("keeps the memory while the visit is the same title", () => {
    // Every re-read of one visit runs this, so it has to be idempotent: a second read that forgot
    // would ask a second time for the same page.
    expect(afterRouteChange(7, 7)).toBe(7);
    expect(afterRouteChange(null, 7)).toBe(null);
  });

  it("forgets when the route is not a title, so coming back is a new visit", () => {
    // The sequence that matters, and the one a ref with a single slot got wrong: title 7, then a
    // screen that is not a title, then title 7 again. Without the forget the second visit could never
    // ask, so a refresh that failed — or a poster that had not landed before the user navigated away
    // — could not be retried without restarting the app.
    const asked = 7;
    const away = afterRouteChange(asked, null);

    expect(away).toBe(null);
    expect(shouldAsk(away, 7, true)).toBe(true);
  });
});

describe("shouldAsk", () => {
  it("asks when the core says a provider could add something", () => {
    expect(shouldAsk(null, 7, true)).toBe(true);
  });

  it("does not ask when the core says it could not", () => {
    // The whole of the rule AGENTS R24 turns on: a title whose provider had no artwork, or one whose
    // poster is already on disk, is nothing to spend a request on.
    expect(shouldAsk(null, 7, false)).toBe(false);
    expect(shouldAsk(7, 7, false)).toBe(false);
  });

  it("does not ask twice for one visit", () => {
    // The read that follows a refresh is the case this exists for: the core's answer is unchanged
    // when the refresh produced no poster, so without the memory every re-read would ask again.
    expect(shouldAsk(7, 7, true)).toBe(false);
  });

  it("asks again on a new visit to the same title", () => {
    // A screen that asked about title 7, moved to title 9, and came back has asked about 9 last.
    expect(shouldAsk(9, 7, true)).toBe(true);
  });
});

describe("followRefresh", () => {
  it("stops following the job whose phase is terminal, and counts it", () => {
    for (const phase of ["succeeded", "failed", "canceled"]) {
      expect(followRefresh(following("abc"), state("abc", phase))).toEqual({
        jobId: null,
        finished: 1,
      });
    }
  });

  it("keeps following a job that has not stopped", () => {
    for (const phase of ["queued", "running", "deferred"]) {
      expect(followRefresh(following("abc"), state("abc", phase))).toEqual(following("abc"));
    }
  });

  it("keeps following when the phase is one this build does not know", () => {
    // A state machine that grew a phase is not this build's to interpret: guessing would end the wait
    // early and re-read before the poster is written.
    expect(followRefresh(following("abc"), state("abc", "paused"))).toEqual(following("abc"));
    expect(followRefresh(following("abc"), state("abc", ""))).toEqual(following("abc"));
  });

  it("ignores another job's event", () => {
    // Two screens can have refreshes in flight; each follows its own.
    expect(followRefresh(following("abc"), state("def", "succeeded"))).toEqual(following("abc"));
  });

  it("ignores an envelope this build cannot read", () => {
    for (const malformed of [
      null,
      undefined,
      "job://state",
      { schemaVersion: 2, payload: { jobId: "abc", phase: "succeeded" } },
      { schemaVersion: 1, payload: "not an object" },
      { schemaVersion: 1, payload: { phase: "succeeded" } },
    ]) {
      expect(followRefresh(following("abc"), malformed)).toEqual(following("abc"));
    }
  });

  it("does nothing at all when no job is being followed", () => {
    // A job this screen does not know about must not move its counter: a re-read that nothing asked
    // for is a read per event on a busy machine.
    expect(followRefresh(NOT_FOLLOWING, state("abc", "succeeded"))).toEqual(NOT_FOLLOWING);
  });

  it("counts every stop, because a screen re-reads when the count moves", () => {
    // A boolean would refresh once and never again for the second refresh this screen watches.
    const once = followRefresh(following("abc"), state("abc", "succeeded"));
    const twice = followRefresh({ ...once, jobId: "def" }, state("def", "failed"));

    expect(twice.finished).toBe(2);
  });
});
