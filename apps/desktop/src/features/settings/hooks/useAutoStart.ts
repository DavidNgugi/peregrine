import { getAutoStartEnabled, normalizeIpcError, setAutoStart } from "@peregrine/ipc-client";
import { useCallback, useEffect, useState } from "react";
import { logUiError } from "../../../lib/error-log.js";

/** The login item's state, as the Settings toggle reads and changes it. */
export interface AutoStartState {
  /** Whether the OS currently has Peregrine registered to start with the session. */
  readonly enabled: boolean;
  /** Whether a change is in flight. */
  readonly busy: boolean;
  /** Whether the last read or change failed, so the toggle can say so. */
  readonly failed: boolean;
}

/** The login item's state and the one action that changes it (PG-E1-13). */
export interface AutoStartControl extends AutoStartState {
  /** Registers or removes the login item, and re-reads what the OS reports. */
  readonly toggle: (next: boolean) => void;
}

/**
 * Reads the login item's state from the OS and changes it when the user asks.
 *
 * The state is re-read after a change rather than assumed from the request: the OS is the
 * authority on whether a login item exists, and on macOS `SMAppService` can refuse without
 * an error the UI could have predicted.
 */
export function useAutoStart(): AutoStartControl {
  const [state, setState] = useState<AutoStartState>({
    enabled: false,
    busy: true,
    failed: false,
  });

  useEffect(() => {
    let cancelled = false;
    getAutoStartEnabled()
      .then((enabled) => {
        if (!cancelled) {
          setState({ enabled, busy: false, failed: false });
        }
      })
      .catch((caught: unknown) => {
        logUiError("autostartEnabled", normalizeIpcError(caught));
        if (!cancelled) {
          setState({ enabled: false, busy: false, failed: true });
        }
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const toggle = useCallback((next: boolean) => {
    setState((current) => ({ ...current, busy: true, failed: false }));
    setAutoStart(next)
      .then(() => getAutoStartEnabled())
      .then((enabled) => {
        setState({ enabled, busy: false, failed: false });
      })
      .catch((caught: unknown) => {
        logUiError("setAutostart", normalizeIpcError(caught));
        setState((current) => ({ ...current, busy: false, failed: true }));
      });
  }, []);

  return { ...state, toggle };
}
