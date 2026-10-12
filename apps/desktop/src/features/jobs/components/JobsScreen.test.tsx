import { createI18n } from "@peregrine/i18n";
import type { Health, IpcError, JobRecord } from "@peregrine/ipc-client";
import { renderToStaticMarkup } from "react-dom/server";
import { I18nextProvider } from "react-i18next";
import { describe, expect, it } from "vitest";
import { jobsStore, type JobsState } from "../job-store.js";
import { JobsScreen } from "./JobsScreen.js";

const i18n = createI18n();

const job: JobRecord = {
  id: 7,
  kind: "hello",
  state: "running",
  priority: 0,
  progressPermille: 250,
  attempts: 1,
  maxAttempts: 3,
  cancelRequested: false,
  createdAt: 1_700_000_000_000,
};

const health: Health = {
  startupMillis: 80,
  scheduler: { ready: 0, deferred: 0, inflight: 1, live: 1, retriesScheduled: 0 },
  events: { published: 4, coalesced: 2, delivered: 3, failed: 2, dropped: 1, degraded: true },
  rescanRequired: false,
};

/** The opening tag of the button whose visible text is `label`. */
function buttonFor(markup: string, label: string): string {
  const match = new RegExp(`<button[^>]*>${label}</button>`, "u").exec(markup);
  return match?.[0]?.slice(0, match[0].indexOf(">") + 1) ?? "";
}

/** Whether a button drawn by the kit is disabled. `disabled:` is a class, so match the attribute. */
function isDisabled(openingTag: string): boolean {
  return / disabled(?:=|>)/u.test(openingTag);
}

/**
 * Draws the screen against a state.
 *
 * The store is vanilla and exported, so the screen is a projection of it and seeding it is
 * enough to say what is drawn. The catch is `zustand` 5: `useStore` hands React
 * `api.getInitialState` as the *server* snapshot, so a static render can only ever see the
 * state the store was constructed with — seeding alone changes nothing, and the screen would
 * be tested against its empty state however it was set up. Swapping the snapshot for the
 * duration of one draw is what makes any other state reachable from here.
 *
 * The override is restored immediately, and it fails loudly rather than quietly: a zustand
 * that stopped reading `getInitialState` would make every assertion below fail, not pass.
 * Testing a store-backed screen through a DOM render is the real answer and needs a test
 * environment and a testing library as declared devDependencies (PG-E21-03).
 */
function draw(state: Partial<JobsState>): string {
  const seeded: JobsState = {
    ...jobsStore.getInitialState(),
    live: null,
    journal: [],
    health: null,
    logLines: [],
    error: null,
    starting: false,
    cancelling: false,
    refreshing: false,
    ...state,
  };

  const snapshot = jobsStore.getInitialState;
  jobsStore.getInitialState = () => seeded;
  try {
    return renderToStaticMarkup(
      <I18nextProvider i18n={i18n}>
        <JobsScreen />
      </I18nextProvider>,
    );
  } finally {
    jobsStore.getInitialState = snapshot;
  }
}

describe("JobsScreen", () => {
  it("names the screen once and its regions below it", () => {
    const markup = draw({});
    expect(markup.match(/<h1/gu)).toHaveLength(1);
    expect(markup.match(/<h2/gu)).toHaveLength(2);
  });

  it("draws a journal row per record, with the figures the core sent", () => {
    const markup = draw({ journal: [job] });

    expect(markup).toContain("hello");
    expect(markup).toContain("running");
    // Permille 250 is 25%, and the row is the record when the event stream coalesced.
    expect(markup).toContain("25%");
    expect(markup).toContain("1/3");
  });

  it("shows the journal's own progress for a job that finished inside one tick", () => {
    // The event stream coalesces, so a job that completed between two ticks never reported
    // a progress value; the journal row is what the bar then shows.
    const markup = draw({
      journal: [{ ...job, state: "succeeded", progressPermille: 1000 }],
      live: {
        jobId: "job-1",
        journalId: 7,
        progressPermille: 0,
        attempt: 1,
        phase: "succeeded",
      },
    });

    expect(markup).toContain("scaleX(1)");
  });

  it("draws an empty state for each region rather than an empty one", () => {
    const markup = draw({});

    expect(markup).toContain("No job has been recorded yet.");
    expect(markup).toContain("Nothing has been logged yet.");
    expect(markup).not.toContain("<table");
    expect(markup).not.toContain("<pre");
  });

  it("reports a failed action as an alert with a way out of it", () => {
    const error: IpcError = {
      code: "STORAGE_BUSY",
      message: "the database is locked",
      retryable: true,
    };
    const markup = draw({ error });

    expect(markup).toContain('role="alert"');
    expect(markup).toContain("The library database is busy.");
    expect(markup).toContain("Dismiss");
  });

  it("says what to do next rather than only what happened", () => {
    // PLAN §8 item 4: a user-facing message says what happened *and* what to do about it,
    // which is the pair the shared mapping carries and a title alone would not.
    const markup = draw({
      error: { code: "STORAGE_BUSY", message: "the database is locked", retryable: true },
    });

    expect(markup).toContain("Another operation is writing to it. Try again in a moment.");
  });

  it("never puts the core's own message on the screen", () => {
    // It is written to the UI log by the store; a message from the core can name a path
    // (SECURITY.md T13), so it does not reach a surface the user copies out of.
    const markup = draw({
      error: {
        code: "STORAGE_BUSY",
        message: "/Users/someone/library.sqlite is locked",
        retryable: true,
      },
    });

    expect(markup).not.toContain("library.sqlite");
  });

  it("says the numbers may lag when the bus dropped events", () => {
    const markup = draw({ health });

    // One dropped and two failed, summed by the core's own counters.
    expect(markup).toContain("3 updates were dropped");
  });

  it("draws the log as preformatted lines rather than as prose", () => {
    const markup = draw({ logLines: ['{"level":"INFO"}', '{"level":"WARN"}'] });

    expect(markup).toContain("<pre");
    // React escapes the JSON's quotes, so the assertion is on the content of the lines and
    // their order rather than on the raw string.
    expect(markup.indexOf("INFO")).toBeLessThan(markup.indexOf("WARN"));
    // One text node with a newline between them: a `<pre>` is the element for a log, and
    // `white-space: pre` is what keeps the line breaks.
    expect(markup).toContain("\n");
  });

  it("offers nothing to cancel until something is running", () => {
    const idle = draw({});
    const running = draw({
      live: { jobId: "job-1", journalId: 7, progressPermille: 100, attempt: 1, phase: "running" },
    });

    // Both screens draw the same three controls; what differs is which of them the screen
    // will accept a press from.
    expect(isDisabled(buttonFor(idle, "Cancel the job"))).toBe(true);
    expect(isDisabled(buttonFor(idle, "Start a hello job"))).toBe(false);
    expect(isDisabled(buttonFor(running, "Cancel the job"))).toBe(false);
    expect(isDisabled(buttonFor(running, "Start a hello job"))).toBe(true);
  });
});
