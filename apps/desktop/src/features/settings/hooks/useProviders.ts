/**
 * The credential fields a settings screen drives.
 *
 * Three things are asked of the core and one answer is followed. The read is the cached status,
 * so it never waits on a keyring; the writes start a job, because proving a key leaves the
 * machine and a screen that blocked on a provider having a slow day would be a screen that froze
 * (`ARCHITECTURE` §7 rule 2). A started job is followed to the phase it ends in, and the reason
 * for a failure is read from the journal row the job wrote — a failed job publishes the phase
 * and never the code (`features/jobs/job-store.ts` refreshes the journal at the same moment, and
 * that is the mechanism this reuses rather than a second one).
 *
 * The envelope rule is `lib/job-events.ts`, shared with the jobs, library, and title screens, so
 * four features cannot disagree about which events are readable. Only the job this hook started
 * is followed: another screen's job is that screen's business.
 *
 * **No rule about a provider is computed here.** Whether a provider can be used is the core's
 * answer, and this hook renders it.
 */

import {
  commands,
  eventTopics,
  normalizeIpcError,
  unwrap,
  type CredentialStoreDto,
  type JobStarted,
  type JobState,
  type ProviderDto,
} from "@peregrine/ipc-client";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { useCallback, useEffect, useRef, useState } from "react";
import { logUiError } from "../../../lib/error-log.js";
import { messageOf } from "../../../lib/ipc-message.js";
import { eventPayload, isTerminal, jobPhase } from "../../../lib/job-events.js";
import {
  credentialFailureKey,
  credentialSuccessKey,
  shouldPoll,
  stillReading,
  type CredentialAction,
} from "../provider-outcome.js";

/** How many journal rows a stopped job is looked for in. */
const JOURNAL_PAGE = 20;

/** What a stopped job left behind, as the screen says it. */
export interface CredentialOutcome {
  /** The catalogue key of the sentence. */
  readonly key: string;
  /** The provider the sentence is about, as the core spells it. */
  readonly provider: string;
  /** Whether the job succeeded, which decides whether the sentence is news or a failure. */
  readonly ok: boolean;
}

/** The providers this build knows, and the three things a person does about a key. */
export interface ProviderSettings {
  /** The known providers, in the order the core answered with. */
  readonly providers: readonly ProviderDto[];
  /**
   * What the last read of the credential store answered.
   *
   * **Not the same question as whether a provider is configured.** A keychain that refused access
   * leaves every `configured` flag false exactly as a first run does, so a section that read the two
   * as one thing would tell a user whose key is still in their keychain to paste it again.
   */
  readonly store: CredentialStoreDto;
  /** The read is still in flight. */
  readonly loading: boolean;
  /** The read failed, so the list is unknown rather than empty. */
  readonly failed: boolean;
  /** A job this hook started is still running, so the controls are refused. */
  readonly busy: boolean;
  /** What the last stopped job left behind, or `null` before one stops. */
  readonly outcome: CredentialOutcome | null;
  /** What the last request failed with before a job could start. */
  readonly error: string | null;
  /** Proves a candidate against the provider and stores it only if it works. */
  readonly saveKey: (provider: string, key: string) => Promise<void>;
  /** Proves the stored key still works. */
  readonly testConnection: (provider: string) => Promise<void>;
  /** Removes whatever is stored, after the caller has confirmed it. */
  readonly removeKey: (provider: string) => Promise<void>;
  /** Asks the core for the credential status again, after a read that has not answered. */
  readonly reload: () => void;
}

/**
 * How long the screen waits between status reads while the keychain has not answered.
 *
 * A second: long enough that the poll is not a spin, and short enough that a read which finishes
 * just after the mount is on screen before a person has read the first sentence.
 */
const POLL_INTERVAL_MS = 1_000;

/** The job being followed, and what started it. */
interface Pending {
  readonly jobId: string;
  readonly journalId: number;
  readonly provider: string;
  readonly action: CredentialAction;
}

interface State {
  readonly providers: readonly ProviderDto[];
  readonly store: CredentialStoreDto;
  readonly loading: boolean;
  readonly failed: boolean;
  readonly busy: boolean;
  readonly outcome: CredentialOutcome | null;
  readonly error: string | null;
}

const INITIAL: State = {
  providers: [],
  // Nobody has read the store yet, which is the state a launch is in until the startup refresh
  // answers. Claiming "not configured" before then would be a claim nothing has established.
  store: "unread",
  loading: true,
  failed: false,
  busy: false,
  outcome: null,
  error: null,
};

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
    logUiError("providers.journal", normalizeIpcError(caught));
    return null;
  }
}

/** The providers the core knows, and the operations the settings screen offers. */
export function useProviders(): ProviderSettings {
  const [state, setState] = useState<State>(INITIAL);
  // How many times this visit has asked again while the keychain had not answered. Bounded by
  // `shouldPoll`, so a keychain that never answers cannot become a poll loop.
  const [attempts, setAttempts] = useState(0);
  // The job being followed, in a ref rather than in state: the listener registered at mount has to
  // see a job started after it, and an event arriving before React has committed the state would
  // otherwise be dropped and the follow would never stop.
  const following = useRef<Pending | null>(null);

  const readStatus = useCallback(async (): Promise<void> => {
    try {
      const answer = await unwrap(commands.status());
      setState((previous) => ({
        ...previous,
        providers: answer.known,
        store: answer.store,
        loading: false,
        failed: false,
      }));
    } catch (caught: unknown) {
      const error = normalizeIpcError(caught);
      logUiError("providers.status", error);
      setState((previous) => ({
        ...previous,
        providers: [],
        loading: false,
        failed: true,
      }));
    }
  }, []);

  /**
   * Asks again, on the user's command.
   *
   * The read can be waiting on a macOS permission dialog, so "reopen the app" is not the only thing a
   * person should be able to do about it: this is the same cached-status read the mount makes, and it
   * is safe to repeat because it never blocks — the blocking half is the runtime's, off the command
   * path.
   */
  const reload = useCallback((): void => {
    setAttempts(0);
    void readStatus();
  }, [readStatus]);

  // While the keychain has not answered, keep asking for a bounded while: the read finishes a moment
  // after a launch, or after a permission dialog is dismissed, and a screen that read once would sit
  // on "reading your keychain" until the user navigated away and back. `shouldPoll` is the bound, and
  // it is a function with its own test because a hook cannot be run in this harness.
  useEffect(() => {
    if (!stillReading(state.store) || !shouldPoll(attempts)) {
      return;
    }
    const timer = setTimeout(() => {
      setAttempts((made) => made + 1);
      void readStatus();
    }, POLL_INTERVAL_MS);
    return () => {
      clearTimeout(timer);
    };
  }, [attempts, readStatus, state.store]);

  useEffect(() => {
    void readStatus();
  }, [readStatus]);

  /** Records what a stopped job left behind, and re-reads what is configured. */
  const finish = useCallback(
    async (pending: Pending, phase: JobState): Promise<void> => {
      const key =
        phase === "succeeded"
          ? credentialSuccessKey(pending.action)
          : credentialFailureKey(await recordedCode(pending.journalId));
      setState((previous) => ({
        ...previous,
        busy: false,
        outcome: { key, provider: pending.provider, ok: phase === "succeeded" },
      }));
      // A key that was stored or removed changes what the core is configured with, so the answer
      // the screen draws is re-read rather than guessed.
      await readStatus();
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
        // The effect was cleaned up before the listener registered; releasing it now is what keeps
        // a visit from leaking one.
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
      action: CredentialAction,
      provider: string,
      call: () => Promise<JobStarted>,
    ): Promise<void> => {
      setState((previous) => ({ ...previous, busy: true, error: null, outcome: null }));
      try {
        const started = await call();
        following.current = {
          jobId: started.jobId,
          journalId: started.journalId,
          provider,
          action,
        };
      } catch (caught: unknown) {
        const error = normalizeIpcError(caught);
        logUiError(`providers.${action}`, error);
        setState((previous) => ({ ...previous, busy: false, error: messageOf(error) }));
      }
    },
    [],
  );

  const saveKey = useCallback(
    (provider: string, key: string) =>
      start("saved", provider, () => unwrap(commands.setKey(provider, key))),
    [start],
  );

  const testConnection = useCallback(
    (provider: string) =>
      start("tested", provider, () => unwrap(commands.testConnection(provider))),
    [start],
  );

  const removeKey = useCallback(
    (provider: string) => start("removed", provider, () => unwrap(commands.clearKey(provider))),
    [start],
  );

  return { ...state, saveKey, testConnection, removeKey, reload };
}
