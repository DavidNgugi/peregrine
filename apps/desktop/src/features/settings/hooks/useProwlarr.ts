/**
 * The managed Prowlarr instance a settings screen drives (docs/adr/0024).
 *
 * Install and start are jobs rather than synchronous calls: the install streams the pinned pack and
 * the start waits for Prowlarr's own health check. The commands answer the job, so this hook polls
 * the status while a job is in flight and stops once the state it asked for arrives. The key never
 * reaches this layer: the core reads Prowlarr's config.xml and stores it.
 */

import {
  installProwlarr,
  normalizeIpcError,
  prowlarrStatus,
  startProwlarr,
  stopProwlarr,
  type ProwlarrStatusDto,
} from "@peregrine/ipc-client";
import { useCallback, useEffect, useState } from "react";
import { logUiError } from "../../../lib/error-log.js";
import { messageOf } from "../../../lib/ipc-message.js";

/** What the last completed action left behind, for the sentence under the controls. */
export type ProwlarrOutcome = "installed" | "started" | "stopped" | null;

/** How often the status is re-read while a job runs. */
const POLL_MILLIS = 2000;
/** How many times, so a job that never lands does not poll for ever (about ten minutes). */
const POLL_LIMIT = 300;

/** The managed instance and the operations the settings screen offers. */
export interface ManagedProwlarr {
  /** The core's answer, or `null` before it has answered. */
  readonly status: ProwlarrStatusDto | null;
  /** The read is still in flight. */
  readonly loading: boolean;
  /** The read failed, so the answer is unknown rather than "not installed". */
  readonly failed: boolean;
  /** An action is in flight, so the controls are refused. */
  readonly busy: boolean;
  /** What the last action did, or `null` before one finishes. */
  readonly outcome: ProwlarrOutcome;
  /** What the last action failed with, in the core's own words. */
  readonly error: string | null;
  /** The effective phase: optimistic while an action begins, else the core's. */
  readonly phase: string;
  /** Install progress in permille (0..=1000), from the core. */
  readonly progressPermille: number;
  /** Downloads, verifies, and installs the pinned pack. */
  readonly install: () => Promise<void>;
  /** Starts the installed instance and stores the key it generated. */
  readonly start: () => Promise<void>;
  /** Stops the running instance. */
  readonly stop: () => Promise<void>;
}

/** The managed Prowlarr instance, and the three things a person does about it. */
export function useProwlarr(): ManagedProwlarr {
  const [status, setStatus] = useState<ProwlarrStatusDto | null>(null);
  const [loading, setLoading] = useState(true);
  const [failed, setFailed] = useState(false);
  const [busy, setBusy] = useState(false);
  const [outcome, setOutcome] = useState<ProwlarrOutcome>(null);
  const [error, setError] = useState<string | null>(null);
  // Set the moment a control is pressed, so the section shows "Installing…" immediately rather
  // than waiting a poll cycle for the core to report the phase.
  const [pending, setPending] = useState<"installing" | "starting" | null>(null);

  const read = useCallback(async (): Promise<ProwlarrStatusDto | null> => {
    try {
      const answer = await prowlarrStatus();
      setStatus(answer);
      setLoading(false);
      setFailed(false);
      return answer;
    } catch (caught: unknown) {
      const normalized = normalizeIpcError(caught);
      logUiError("prowlarr.status", normalized);
      setLoading(false);
      setFailed(true);
      return null;
    }
  }, []);

  useEffect(() => {
    void read();
  }, [read]);

  const waitFor = useCallback(
    async (done: (state: ProwlarrStatusDto) => boolean): Promise<boolean> => {
      for (let attempt = 0; attempt < POLL_LIMIT; attempt += 1) {
        await new Promise((resolve) => setTimeout(resolve, POLL_MILLIS));
        const answer = await read();
        if (answer && done(answer)) {
          return true;
        }
      }
      return false;
    },
    [read],
  );

  const action = useCallback(
    async (
      call: () => Promise<unknown>,
      done: ProwlarrOutcome,
      tag: string,
      settled: (state: ProwlarrStatusDto) => boolean,
      startedAs: "installing" | "starting",
    ): Promise<void> => {
      setBusy(true);
      setPending(startedAs);
      setError(null);
      setOutcome(null);
      try {
        await call();
        if (await waitFor(settled)) {
          setOutcome(done);
        } else {
          setError("The managed Prowlarr did not reach the expected state in time.");
        }
      } catch (caught: unknown) {
        const normalized = normalizeIpcError(caught);
        logUiError(tag, normalized);
        setError(messageOf(normalized));
      } finally {
        setBusy(false);
        setPending(null);
        await read();
      }
    },
    [read, waitFor],
  );

  const install = useCallback(
    () =>
      action(
        () => installProwlarr(),
        "installed",
        "prowlarr.install",
        (state) => state.installed,
        "installing",
      ),
    [action],
  );

  const start = useCallback(
    () =>
      action(
        () => startProwlarr(),
        "started",
        "prowlarr.start",
        (state) => state.running,
        "starting",
      ),
    [action],
  );

  const stop = useCallback(async (): Promise<void> => {
    setBusy(true);
    setError(null);
    setOutcome(null);
    try {
      setStatus(await stopProwlarr());
      setOutcome("stopped");
    } catch (caught: unknown) {
      const normalized = normalizeIpcError(caught);
      logUiError("prowlarr.stop", normalized);
      setError(messageOf(normalized));
    } finally {
      setBusy(false);
    }
  }, []);

  const phase = pending ?? status?.phase ?? "idle";
  const progressPermille = status?.progressPermille ?? 0;
  return {
    status,
    loading,
    failed,
    busy,
    outcome,
    error,
    phase,
    progressPermille,
    install,
    start,
    stop,
  };
}
