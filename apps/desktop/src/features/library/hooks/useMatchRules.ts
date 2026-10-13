/**
 * The hook layer for the rules a user's corrections became.
 *
 * A rule is an answer that keeps acting: it places every later name that reads the way the one it was
 * made from did, without asking again. That is the point of `PG-E14-09` and also the reason this hook
 * exists — an answer a user cannot see is an answer they cannot withdraw, and a wrong rule would go
 * on placing files silently.
 *
 * Three states again, for the reason the other reads give: a rules list that failed to load and a
 * library with no rules look identical on screen, and "you have no rules" is the wrong thing to say
 * when the read did not answer.
 */

import { forgetMatchRule, listMatchRules, normalizeIpcError } from "@peregrine/ipc-client";
import type { IpcError, MatchRuleDto } from "@peregrine/ipc-client";
import { useCallback, useEffect, useState } from "react";
import { logUiError } from "../../../lib/error-log.js";
import { messageOf } from "../../../lib/ipc-message.js";

/** The rules, and what the screen needs to know about the read. */
export interface MatchRules {
  /** The rules in force, newest first. */
  readonly rules: readonly MatchRuleDto[];
  /** The read is still in flight. */
  readonly loading: boolean;
  /** The read failed, so the list is unknown rather than empty. */
  readonly failed: boolean;
  /** A removal is in flight, so the buttons that remove are disabled. */
  readonly busy: boolean;
  /** What the last removal failed with, in the core's own words. */
  readonly error: string | null;
  /** Takes one rule back. The files it placed stay. */
  readonly forget: (id: number) => Promise<void>;
  /** Re-reads the list. */
  readonly refresh: () => Promise<void>;
}

/** The rules in force, and the action that takes one back. */
export function useMatchRules(): MatchRules {
  const [state, setState] = useState<{
    rules: readonly MatchRuleDto[];
    loading: boolean;
    failed: boolean;
    busy: boolean;
    error: string | null;
  }>({ rules: [], loading: true, failed: false, busy: false, error: null });

  const refresh = useCallback(async (): Promise<void> => {
    try {
      const rules = await listMatchRules();
      setState((previous) => ({ ...previous, rules, loading: false, failed: false }));
    } catch (caught: unknown) {
      const error: IpcError = normalizeIpcError(caught);
      logUiError("listMatchRules", error);
      setState((previous) => ({ ...previous, rules: [], loading: false, failed: true }));
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const forget = useCallback(
    async (id: number): Promise<void> => {
      setState((previous) => ({ ...previous, busy: true, error: null }));
      try {
        await forgetMatchRule(id);
        await refresh();
      } catch (caught: unknown) {
        const error = normalizeIpcError(caught);
        logUiError("forgetMatchRule", error);
        setState((previous) => ({ ...previous, error: messageOf(error) }));
      } finally {
        setState((previous) => ({ ...previous, busy: false }));
      }
    },
    [refresh],
  );

  return { ...state, forget, refresh };
}
