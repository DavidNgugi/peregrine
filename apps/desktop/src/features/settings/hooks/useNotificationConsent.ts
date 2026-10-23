/**
 * The notification consent the Settings screen reads and writes.
 *
 * The core owns the answer (AGENTS R7): this hook only carries it between the stored preference
 * and the toggle. Granting is the one path that asks the operating system, because the plugin's
 * desktop permission is the system's to give and a refusal has to be recorded as one (R31).
 */

import {
  normalizeIpcError,
  notificationPreferences,
  setNotificationConsent,
  type NotificationPreferencesDto,
} from "@peregrine/ipc-client";
import { useCallback, useEffect, useState } from "react";

import { logUiError } from "../../../lib/error-log.js";
import {
  probePermission,
  requestPermission,
  type PermissionProbe,
} from "../../../lib/notifications.js";

/** One write's result: the stored answer and what the OS said about its own permission. */
export interface NotificationConsentWrite {
  readonly preferences: NotificationPreferencesDto;
  readonly permission: PermissionProbe;
}

/**
 * Asks the OS only when consent is being granted, then records the effective answer.
 *
 * A denial from the system is stored as a refusal rather than as an intent: a consent the OS will
 * not honour would make every event's toast attempt fail while the centre row pretended otherwise.
 * Withdrawing consent never asks the system anything.
 */
export async function writeNotificationConsent(
  granted: boolean,
  request: () => Promise<PermissionProbe>,
  save: (granted: boolean) => Promise<NotificationPreferencesDto>,
): Promise<NotificationConsentWrite> {
  if (!granted) {
    return { preferences: await save(false), permission: { kind: "denied" } };
  }
  const permission = await request();
  return {
    preferences: await save(permission.kind === "granted"),
    permission,
  };
}

/** The consent state the section draws. */
export interface NotificationConsentState {
  /** The stored preference, or null before the first read answers. */
  readonly preferences: NotificationPreferencesDto | null;
  /** What the OS reports about its own permission, without prompting. */
  readonly permission: PermissionProbe;
  /** The first read is still running. */
  readonly loading: boolean;
  /** A write is in flight. */
  readonly busy: boolean;
  /** The first read failed. */
  readonly failed: boolean;
  /** The last write failed. */
  readonly saveFailed: boolean;
  /** The user asked to grant, and the OS did not. */
  readonly denied: boolean;
}

/** The state and the one action that changes it. */
export interface NotificationConsentControl extends NotificationConsentState {
  /** Records the user's answer, asking the OS when it is a grant. */
  readonly toggle: (granted: boolean) => void;
}

const INITIAL: NotificationConsentState = {
  preferences: null,
  permission: { kind: "unverified" },
  loading: true,
  busy: false,
  failed: false,
  saveFailed: false,
  denied: false,
};

/** Reads the stored consent and changes it when the user asks. */
export function useNotificationConsent(): NotificationConsentControl {
  const [state, setState] = useState<NotificationConsentState>(INITIAL);

  useEffect(() => {
    let cancelled = false;
    void Promise.all([notificationPreferences(), probePermission()])
      .then(([preferences, permission]) => {
        if (!cancelled) {
          setState({ ...INITIAL, preferences, permission, loading: false });
        }
      })
      .catch((caught: unknown) => {
        logUiError("notificationPreferences", normalizeIpcError(caught));
        if (!cancelled) {
          setState({ ...INITIAL, loading: false, failed: true });
        }
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const toggle = useCallback((granted: boolean): void => {
    setState((previous) => ({ ...previous, busy: true, saveFailed: false, denied: false }));
    void writeNotificationConsent(granted, requestPermission, setNotificationConsent)
      .then(({ preferences, permission }) => {
        setState({
          preferences,
          permission,
          loading: false,
          busy: false,
          failed: false,
          saveFailed: false,
          denied: granted && permission.kind !== "granted",
        });
      })
      .catch((caught: unknown) => {
        logUiError("setNotificationConsent", normalizeIpcError(caught));
        setState((previous) => ({ ...previous, busy: false, saveFailed: true }));
      });
  }, []);

  return { ...state, toggle };
}
