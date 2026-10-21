/**
 * Writing a backup and staging a restore, for the Settings screen.
 *
 * The screen chooses the folder or the file because the OS dialog is the only way this app
 * produces a path, and the core is what validates it. A backup resolves and reports where it
 * went; a restore deliberately does not resolve in the ordinary sense, because the app is
 * restarted as the command returns (ADR 0033): the hook keeps "restoring" on screen and the
 * next launch opens the restored database.
 */

import { createBackup, normalizeIpcError, restoreBackup } from "@peregrine/ipc-client";
import type { BackupDto, RestoreDto } from "@peregrine/ipc-client";
import { useCallback, useState } from "react";
import { logUiError } from "../../../lib/error-log.js";
import { messageOf } from "../../../lib/ipc-message.js";
import { pickFolder } from "../../../lib/picker.js";

/** What the screen should say after an attempt. */
export type BackupOutcome =
  | { readonly kind: "created"; readonly backup: BackupDto }
  | { readonly kind: "restoring"; readonly restore: RestoreDto };

/** The two operations and the state a Settings section draws. */
export interface BackupControl {
  /** A backup or restore is in flight. */
  readonly busy: boolean;
  /** The last success, or null before one. */
  readonly outcome: BackupOutcome | null;
  /** What the last attempt failed with, ready to show. */
  readonly error: string | null;
  /** Opens the folder picker and writes a backup there; false when nothing was written. */
  readonly create: () => Promise<boolean>;
  /** Stages one chosen file and asks the app to restart; false when it was refused. */
  readonly restore: (source: string) => Promise<boolean>;
}

/** The backup and restore actions. */
export function useBackup(): BackupControl {
  const [state, setState] = useState<{
    busy: boolean;
    outcome: BackupOutcome | null;
    error: string | null;
  }>({ busy: false, outcome: null, error: null });

  const create = useCallback(async (): Promise<boolean> => {
    const picked = await pickFolder();
    if (picked === null || picked.kind !== "folder") {
      return false;
    }
    setState((previous) => ({ ...previous, busy: true, error: null }));
    try {
      const backup = await createBackup(picked.path);
      setState({ busy: false, outcome: { kind: "created", backup }, error: null });
      return true;
    } catch (caught: unknown) {
      const error = normalizeIpcError(caught);
      logUiError("createBackup", error);
      setState((previous) => ({
        ...previous,
        busy: false,
        error: messageOf(error),
      }));
      return false;
    }
  }, []);

  const restore = useCallback(async (source: string): Promise<boolean> => {
    setState((previous) => ({ ...previous, busy: true, error: null }));
    try {
      const answer = await restoreBackup(source);
      // The process is restarting. Leave the section busy and say so; the promise may never
      // settle if the exit wins the race, which is a success we cannot observe from here.
      setState({
        busy: true,
        outcome: { kind: "restoring", restore: answer },
        error: null,
      });
      return true;
    } catch (caught: unknown) {
      const error = normalizeIpcError(caught);
      logUiError("restoreBackup", error);
      setState((previous) => ({
        ...previous,
        busy: false,
        error: messageOf(error),
      }));
      return false;
    }
  }, []);

  return { ...state, create, restore };
}
