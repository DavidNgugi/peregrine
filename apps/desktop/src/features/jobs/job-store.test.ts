import { beforeEach, describe, expect, it, vi } from "vitest";

/**
 * The store's actions against a stubbed transport.
 *
 * The commands module is mocked rather than the Tauri bridge, so these tests exercise the
 * store's own logic: what it does with a failure, and when it decides the journal needs
 * rereading. `unwrap` is kept real, so a failure travels through the same normalization
 * the app uses.
 */
const startHello = vi.fn();
const cancel = vi.fn();
const list = vi.fn();
const health = vi.fn();
const logTail = vi.fn();

vi.mock("@peregrine/ipc-client", async () => {
  const actual = await vi.importActual("@peregrine/ipc-client");
  return {
    ...actual,
    commands: { startHello, cancel, list, health, logTail },
  };
});

const { jobsStore } = await import("./job-store.js");

const JOB = "01ARZ3NDEKTSV4RRFFQ69G5FAV";

const ROW = {
  id: 3,
  kind: "hello",
  state: "running",
  priority: 100,
  progressPermille: 250,
  attempts: 1,
  maxAttempts: 1,
  createdAt: 1_700_000_000_000,
  cancelRequested: false,
} as const;

const HEALTH = {
  startupMillis: 12,
  scheduler: { ready: 0, deferred: 0, inflight: 0, live: 0, retriesScheduled: 0 },
  events: {
    published: 4,
    coalesced: 2,
    delivered: 2,
    failed: 0,
    dropped: 0,
    degraded: false,
  },
} as const;

function ok<T>(data: T): Promise<{ status: "ok"; data: T }> {
  return Promise.resolve({ status: "ok", data });
}

function fail(code: string): Promise<{ status: "error"; error: unknown }> {
  return Promise.resolve({
    status: "error",
    error: { code, message: "it failed", retryable: false },
  });
}

beforeEach(() => {
  vi.clearAllMocks();
  jobsStore.setState({
    live: null,
    journal: [],
    health: null,
    logLines: [],
    error: null,
    starting: false,
    cancelling: false,
    refreshing: false,
  });
  list.mockReturnValue(ok([ROW]));
  health.mockReturnValue(ok(HEALTH));
  logTail.mockReturnValue(ok(['{"message":"hello"}']));
});

describe("start", () => {
  it("records the accepted job and reads the journal back", async () => {
    startHello.mockReturnValue(ok({ jobId: JOB, journalId: 3 }));

    await jobsStore.getState().start(300);

    const state = jobsStore.getState();
    expect(startHello).toHaveBeenCalledWith(300);
    expect(state.live).toMatchObject({ jobId: JOB, journalId: 3, phase: "queued" });
    expect(state.journal).toHaveLength(1);
    expect(state.health).toEqual(HEALTH);
    expect(state.logLines).toEqual(['{"message":"hello"}']);
    expect(state.starting).toBe(false);
    expect(state.error).toBeNull();
  });

  it("reports a refused request and starts nothing", async () => {
    startHello.mockReturnValue(fail("INVALID_INPUT"));

    await jobsStore.getState().start(0);

    const state = jobsStore.getState();
    expect(state.live).toBeNull();
    expect(state.error).toMatchObject({ code: "INVALID_INPUT" });
    expect(state.starting).toBe(false);
  });
});

describe("cancel", () => {
  it("cancels the live job and leaves the terminal phase to the event stream", async () => {
    startHello.mockReturnValue(ok({ jobId: JOB, journalId: 3 }));
    cancel.mockReturnValue(ok(null));
    await jobsStore.getState().start(300);

    await jobsStore.getState().cancel();

    expect(cancel).toHaveBeenCalledWith(JOB);
    expect(jobsStore.getState().cancelling).toBe(false);
    expect(jobsStore.getState().error).toBeNull();
  });

  it("does nothing when no job is live", async () => {
    await jobsStore.getState().cancel();
    expect(cancel).not.toHaveBeenCalled();
  });

  it("reports a cancellation the core refused", async () => {
    startHello.mockReturnValue(ok({ jobId: JOB, journalId: 3 }));
    cancel.mockReturnValue(fail("NOT_FOUND"));
    await jobsStore.getState().start(300);

    await jobsStore.getState().cancel();

    expect(jobsStore.getState().error).toMatchObject({ code: "NOT_FOUND" });
  });
});

describe("refresh", () => {
  it("reports a failure without discarding what it already had", async () => {
    list.mockReturnValue(fail("STORAGE_CORRUPT"));

    await jobsStore.getState().refresh();

    const state = jobsStore.getState();
    expect(state.error).toMatchObject({ code: "STORAGE_CORRUPT" });
    expect(state.refreshing).toBe(false);
    expect(state.journal).toEqual([]);
  });
});

describe("apply", () => {
  it("moves the live job and rereads the journal when the job is terminal", async () => {
    startHello.mockReturnValue(ok({ jobId: JOB, journalId: 3 }));
    await jobsStore.getState().start(300);
    list.mockClear();

    jobsStore.getState().apply({
      kind: "state",
      job: { jobId: JOB, journalId: 3, progressPermille: 0, attempt: 1, phase: "succeeded" },
      phase: "succeeded",
    });

    expect(jobsStore.getState().live?.phase).toBe("succeeded");
    // The refresh is asynchronous; a microtask is enough for the mocked commands.
    await Promise.resolve();
    await Promise.resolve();
    expect(list).toHaveBeenCalled();
  });

  it("does not reread the journal for a job that is still running", async () => {
    startHello.mockReturnValue(ok({ jobId: JOB, journalId: 3 }));
    await jobsStore.getState().start(300);
    list.mockClear();

    jobsStore.getState().apply({
      kind: "progress",
      job: { jobId: JOB, journalId: 3, progressPermille: 500, attempt: 1, phase: null },
    });

    await Promise.resolve();
    expect(list).not.toHaveBeenCalled();
    expect(jobsStore.getState().live?.progressPermille).toBe(500);
  });
});
