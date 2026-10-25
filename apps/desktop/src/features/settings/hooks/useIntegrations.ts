/**
 * The integrations a settings screen drives, and the four things a person changes about one.
 *
 * **The core owns every rule.** Whether an integration is configured, what the last probe
 * answered, and whether it needs attention are all answers this hook renders rather than
 * computes from a manifest of its own. The one rule that lives here is the `needsAttention`
 * decision, and it is the rule the DTO leaves the screen to spell out: a refused probe or a
 * required credential that is missing is the set of states a person has to act on.
 *
 * **A job is followed, not waited on.** A connection test, a credential write, and a credential
 * removal all leave the machine, so each answers `JobStarted`; the follow is the one
 * `lib/job-events.ts` defines, shared with the provider, library, and title screens. Only the
 * job this hook started is followed, and only one is followed at a time — a double click starts
 * one job, not two.
 *
 * **A key is never read back.** `field.present` is the whole of what this hook learns about a
 * stored value; the candidate in the field is handed to the command and dropped.
 */

import {
  commands,
  eventTopics,
  normalizeIpcError,
  unwrap,
  type JobStarted,
  type JobState,
} from "@peregrine/ipc-client";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { useCallback, useEffect, useRef, useState } from "react";
import { logUiError } from "../../../lib/error-log.js";
import { messageOf } from "../../../lib/ipc-message.js";
import { eventPayload, isTerminal, jobPhase } from "../../../lib/job-events.js";

/** How many journal rows a stopped job is looked for in. */
const JOURNAL_PAGE = 20;

/** The payload of a generated command's successful result. */
type Succeeded<T> = T extends { readonly status: "ok"; readonly data: infer Data } ? Data : never;

/**
 * One integration row, as the generated binding spells it.
 *
 * The IPC package does not re-export its generated integration types yet, so this names the
 * command's own answer rather than a hand-written copy of the contract: a field that changes in
 * Rust changes this type at the next generation, and a second table cannot drift from the first.
 */
export type IntegrationDto = Succeeded<
  Awaited<ReturnType<typeof commands.listIntegrations>>
>[number];

/** One credential candidate, named by the generated command's own parameter. */
export type CredentialValue = Parameters<typeof commands.setIntegrationCredentials>[1][number];

/** What the last bounded probe answered. */
export type IntegrationProbeOutcome = NonNullable<IntegrationDto["lastOutcome"]>;

/** The three things a credential job is started for. */
export type IntegrationAction = "tested" | "saved" | "removed";

/** Which way a priority move goes. */
export type PriorityDirection = "up" | "down";

/** The argument one priority move hands `setIntegrationPriority`. */
export interface IntegrationMove {
  readonly id: string;
  readonly priority: number;
  /** The neighbour's row and the value it takes, so a move is a swap rather than a tie. */
  readonly swapId: string;
  readonly swapPriority: number;
}

/** What a stopped job left behind, as the section says it. */
export interface IntegrationOutcome {
  /** The catalogue key of the sentence. */
  readonly key: string;
  /** The integration the sentence is about. */
  readonly integration: string;
  /** Whether the job succeeded, which decides whether the sentence is news or a failure. */
  readonly ok: boolean;
}

/** The integrations this build knows, and the actions the settings screen offers. */
export interface IntegrationSettings {
  /** Every integration in the core's order, which is the manifest's. */
  readonly integrations: readonly IntegrationDto[];
  /** The read is still in flight. */
  readonly loading: boolean;
  /** The read failed, so the list is unknown rather than empty. */
  readonly failed: boolean;
  /** A job or a write this hook started is still running, so the controls are refused. */
  readonly busy: boolean;
  /** What the last stopped job left behind, or `null` before one stops. */
  readonly outcome: IntegrationOutcome | null;
  /** The job still running, so the screen can say what it is waiting for and where. */
  readonly pending: {
    readonly integration: string;
    readonly action: IntegrationAction;
  } | null;
  /** What the last request failed with before it could start. */
  readonly error: string | null;
  /** Asks the core for the list again, after a read that has not answered. */
  readonly reload: () => void;
  /** Enables or disables one integration. */
  readonly setEnabled: (id: string, enabled: boolean) => Promise<void>;
  /** Records or withdraws consent for one integration's submissions. */
  readonly setSubmission: (id: string, enabled: boolean) => Promise<void>;
  /** Moves one integration one place within its category. */
  readonly move: (id: string, direction: PriorityDirection) => Promise<void>;
  /** Proves the stored credential against the service. */
  readonly testConnection: (id: string) => Promise<void>;
  /** Proves and stores the non-empty candidates for one integration. */
  readonly saveCredentials: (id: string, values: readonly CredentialValue[]) => Promise<void>;
  /** Removes every stored value for one integration. */
  readonly removeCredentials: (id: string) => Promise<void>;
}

/**
 * The catalogue key of one integration's state, from its flag and what the keychain answered.
 *
 * **A refused read is not an absent key.** A keychain that refused access leaves every integration
 * unconfigured exactly as a first run does, so a card that read the flag alone would say "Not
 * configured" to someone whose credential is still in their keychain. The flag still wins when it
 * is set, because an adapter built from a value that was read once is still in hand.
 */
export function integrationStateKey(configured: boolean, store: IntegrationDto["store"]): string {
  if (configured) {
    return "settings.integrations.configured";
  }
  switch (store) {
    case "answered":
      return "settings.integrations.notConfigured";
    case "denied":
      return "settings.integrations.store.denied";
    case "unavailable":
      return "settings.integrations.store.unavailable";
    case "unread":
      return "settings.integrations.store.unread";
  }
}

/** The catalogue key of what the last bounded probe answered. */
export function lastOutcomeKey(outcome: IntegrationProbeOutcome): string {
  switch (outcome) {
    case "working":
      return "settings.integrations.lastOutcome.working";
    case "unauthorized":
      return "settings.integrations.lastOutcome.unauthorized";
    case "rateLimited":
      return "settings.integrations.lastOutcome.rateLimited";
    case "unreachable":
      return "settings.integrations.lastOutcome.unreachable";
    case "keychainDenied":
      return "settings.integrations.lastOutcome.keychainDenied";
    case "notConfigured":
      return "settings.integrations.lastOutcome.notConfigured";
  }
}

/** Whether a card has to draw the attention state rather than the ordinary one. */
export function needsAttention(entry: IntegrationDto): boolean {
  return entry.needsAttention;
}

/**
 * The `setIntegrationPriority` argument for moving one integration one place, or `null` at the
 * end of its category.
 *
 * The move takes the neighbour's priority value rather than a computed one — the core stores one
 * priority per integration, and the neighbour's value is the place the moved row takes. The
 * neighbour keeps its own value, which is what the task asks for and what the next read draws.
 */
export function priorityMove(
  rows: readonly IntegrationDto[],
  id: string,
  direction: PriorityDirection,
): IntegrationMove | null {
  const row = rows.find((candidate) => candidate.id === id);
  if (row === undefined) {
    return null;
  }
  const category = rows
    .filter((candidate) => candidate.category === row.category)
    .sort((left, right) => left.priority - right.priority);
  const index = category.findIndex((candidate) => candidate.id === id);
  if (index < 0) {
    return null;
  }
  const neighbour = direction === "up" ? category[index - 1] : category[index + 1];
  if (neighbour === undefined) {
    return null;
  }
  return {
    id,
    priority: neighbour.priority,
    swapId: neighbour.id,
    swapPriority: row.priority,
  };
}

/** The catalogue key of what a job that succeeded did, by the action it was started for. */
const SUCCESS_KEYS: Readonly<Record<IntegrationAction, string>> = {
  tested: "settings.integrations.outcome.tested",
  saved: "settings.integrations.outcome.saved",
  removed: "settings.integrations.outcome.removed",
};

/** The catalogue key of what a successful job did. */
export function integrationSuccessKey(action: IntegrationAction): string {
  return SUCCESS_KEYS[action];
}

/**
 * The catalogue key of why an integration job failed.
 *
 * A code this build does not know is answered like a failure with no code at all: the journal is
 * the record, and turning an unrecognised string into a specific instruction would be guessing at
 * what the service said.
 */
export function integrationFailureKey(code: string | null | undefined): string {
  switch (code) {
    case "INTEGRATION_UNAUTHORIZED":
      return "settings.integrations.outcome.unauthorized";
    case "INTEGRATION_RATE_LIMITED":
      return "settings.integrations.outcome.rateLimited";
    case "INTEGRATION_UNREACHABLE":
      return "settings.integrations.outcome.unreachable";
    case "INTEGRATION_NOT_CONFIGURED":
      return "settings.integrations.outcome.notConfigured";
    case "INTEGRATION_STORE":
      return "settings.integrations.outcome.store";
    case "CANCELLED":
      return "settings.integrations.outcome.cancelled";
    default:
      return "settings.integrations.outcome.failed";
  }
}

interface State {
  readonly integrations: readonly IntegrationDto[];
  readonly loading: boolean;
  readonly failed: boolean;
  readonly busy: boolean;
  readonly outcome: IntegrationOutcome | null;
  readonly pending: {
    readonly integration: string;
    readonly action: IntegrationAction;
  } | null;
  readonly error: string | null;
}

const INITIAL: State = {
  integrations: [],
  loading: true,
  failed: false,
  busy: false,
  outcome: null,
  pending: null,
  error: null,
};

/** The job being followed, and what started it. */
interface Pending {
  readonly jobId: string;
  readonly journalId: number;
  readonly integration: string;
  readonly action: IntegrationAction;
}

/**
 * The stable code a stopped job recorded, read from the journal row it wrote.
 *
 * The row is the record: the job publishes the phase it ended in and no code, so the reason is
 * where `jobs::list` can find it. A row that cannot be read is answered as no code at all, which
 * is the generic sentence rather than a claim about what failed.
 */
async function recordedCode(journalId: number): Promise<string | null> {
  try {
    const rows = await unwrap(commands.list(JOURNAL_PAGE));
    return rows.find((row) => row.id === journalId)?.lastErrorCode ?? null;
  } catch (caught: unknown) {
    logUiError("integrations.journal", normalizeIpcError(caught));
    return null;
  }
}

/** The integrations the core knows, and the operations the settings screen offers. */
export function useIntegrations(): IntegrationSettings {
  const [state, setState] = useState<State>(INITIAL);
  // One action at a time. A ref rather than the state flag because a double click arrives before
  // React has committed the disabled control, and the second press would otherwise start a second
  // job for the same integration.
  const inFlight = useRef(false);
  // The job being followed, in a ref rather than in state: the listener registered at mount has to
  // see a job started after it, and an event arriving before React has committed the state would
  // otherwise be dropped and the follow would never stop.
  const following = useRef<Pending | null>(null);

  const readStatus = useCallback(async (): Promise<void> => {
    try {
      const rows = await unwrap(commands.listIntegrations());
      setState((previous) => ({
        ...previous,
        integrations: rows,
        loading: false,
        failed: false,
      }));
    } catch (caught: unknown) {
      const error = normalizeIpcError(caught);
      logUiError("integrations.list", error);
      setState((previous) => ({
        ...previous,
        integrations: [],
        loading: false,
        failed: true,
      }));
    }
  }, []);

  const reload = useCallback((): void => {
    setState((previous) => ({ ...previous, loading: true, failed: false }));
    void readStatus();
  }, [readStatus]);

  useEffect(() => {
    void readStatus();
  }, [readStatus]);

  /** Records what a stopped job left behind, and re-reads what is configured. */
  const finish = useCallback(
    async (pending: Pending, phase: JobState): Promise<void> => {
      const key =
        phase === "succeeded"
          ? integrationSuccessKey(pending.action)
          : integrationFailureKey(await recordedCode(pending.journalId));
      await readStatus();
      inFlight.current = false;
      setState((previous) => ({
        ...previous,
        busy: false,
        pending: null,
        outcome: { key, integration: pending.integration, ok: phase === "succeeded" },
      }));
    },
    [readStatus],
  );

  useEffect(() => {
    let cancelled = false;
    let unlisten: UnlistenFn | null = null;

    void listen<unknown>(eventTopics.jobState.name, (event) => {
      const payload = eventPayload(event.payload);
      const current = following.current;
      if (payload === null || current === null || payload["jobId"] !== current.jobId) {
        return;
      }
      const phase = jobPhase(payload["phase"]);
      if (phase === null || !isTerminal(phase)) {
        return;
      }
      // Released before the read, so a later event about the same id cannot start a second follow.
      following.current = null;
      void finish(current, phase);
    }).then((release) => {
      if (cancelled) {
        release();
      } else {
        unlisten = release;
      }
    });

    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [finish]);

  const start = useCallback(
    async (
      action: IntegrationAction,
      integration: string,
      call: () => Promise<JobStarted>,
    ): Promise<void> => {
      if (inFlight.current) {
        return;
      }
      inFlight.current = true;
      setState((previous) => ({
        ...previous,
        busy: true,
        error: null,
        outcome: null,
        pending: { integration, action },
      }));
      try {
        const started = await call();
        following.current = {
          jobId: started.jobId,
          journalId: started.journalId,
          integration,
          action,
        };
      } catch (caught: unknown) {
        const error = normalizeIpcError(caught);
        logUiError(`integrations.${action}`, error);
        inFlight.current = false;
        setState((previous) => ({
          ...previous,
          busy: false,
          pending: null,
          error: messageOf(error),
        }));
      }
    },
    [],
  );

  /** A write that is not a job: the preference commands answer when the database has accepted. */
  const write = useCallback(
    async (call: () => Promise<unknown>, tag: string): Promise<void> => {
      if (inFlight.current) {
        return;
      }
      inFlight.current = true;
      setState((previous) => ({
        ...previous,
        busy: true,
        error: null,
        outcome: null,
        pending: null,
      }));
      try {
        await call();
      } catch (caught: unknown) {
        const error = normalizeIpcError(caught);
        logUiError(tag, error);
        setState((previous) => ({ ...previous, error: messageOf(error) }));
      } finally {
        await readStatus();
        inFlight.current = false;
        setState((previous) => ({ ...previous, busy: false }));
      }
    },
    [readStatus],
  );

  const setEnabled = useCallback(
    (id: string, enabled: boolean) =>
      write(() => unwrap(commands.setIntegrationEnabled(id, enabled)), "integrations.enabled"),
    [write],
  );

  const setSubmission = useCallback(
    (id: string, enabled: boolean) =>
      write(
        () => unwrap(commands.setIntegrationSubmission(id, enabled)),
        "integrations.submission",
      ),
    [write],
  );

  const move = useCallback(
    (id: string, direction: PriorityDirection): Promise<void> => {
      const target = priorityMove(state.integrations, id, direction);
      if (target === null) {
        return Promise.resolve();
      }
      return write(async () => {
        await unwrap(commands.setIntegrationPriority(target.id, target.priority));
        await unwrap(commands.setIntegrationPriority(target.swapId, target.swapPriority));
      }, "integrations.priority");
    },
    [state.integrations, write],
  );

  const testConnection = useCallback(
    (id: string) => start("tested", id, () => unwrap(commands.testIntegration(id))),
    [start],
  );

  const saveCredentials = useCallback(
    (id: string, values: readonly CredentialValue[]) =>
      start("saved", id, () => unwrap(commands.setIntegrationCredentials(id, [...values]))),
    [start],
  );

  const removeCredentials = useCallback(
    (id: string) => start("removed", id, () => unwrap(commands.clearIntegrationCredentials(id))),
    [start],
  );

  return {
    ...state,
    reload,
    setEnabled,
    setSubmission,
    move,
    testConnection,
    saveCredentials,
    removeCredentials,
  };
}
