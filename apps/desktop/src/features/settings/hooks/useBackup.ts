/**
 * Writing a backup and staging a restore, for the Settings screen.
 *
 * The screen chooses the folder or the file because the OS dialog is the only way this app
 * produces a path, and the core is what validates it. A backup resolves and reports where it
 * went; a restore deliberately does not resolve in the ordinary sense, because the app is
 * restarted as the command returns (ADR 0033): the hook keeps "restoring" on screen and the
 * next launch opens the restored database.
 */

import { isTauri } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { createBackup, normalizeIpcError, restoreBackup } from "@peregrine/ipc-client";
import type { BackupDto, RestoreDto } from "@peregrine/ipc-client";
import { useCallback, useEffect, useState } from "react";
import { logUiError } from "../../../lib/error-log.js";
import { messageOf } from "../../../lib/ipc-message.js";
import { eventPayload } from "../../../lib/job-events.js";
import { pickFolder } from "../../../lib/picker.js";

/** What the screen should say after an attempt. */
export type BackupOutcome =
  | { readonly kind: "created"; readonly backup: BackupDto }
  | { readonly kind: "restoring"; readonly restore: RestoreDto };

/** One phase of the operation in flight, as the core reports it. */
export interface BackupProgress {
  readonly operation: "backup" | "restore";
  /** The stable phase spelling the core emitted: copying, cleaning, compacting, staging, … */
  readonly phase: string;
  /** The phase's share of the whole, 0 to 1000. */
  readonly permille: number;
}

/** The two operations and the state a Settings section draws. */
export interface BackupControl {
  /** A backup or restore is in flight. */
  readonly busy: boolean;
  /** The last success, or null before one. */
  readonly outcome: BackupOutcome | null;
  /** What the last attempt failed with, ready to show. */
  readonly error: string | null;
  /** Which action is running, or null when nothing is. */
  readonly active: "backup" | "restore" | null;
  /** The phase currently running, or null before the core's first report. */
  readonly progress: BackupProgress | null;
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
    active: "backup" | "restore" | null;
    progress: BackupProgress | null;
  }>({ busy: false, outcome: null, error: null, active: null, progress: null });

  useEffect(() => {
    if (!isTauri()) {
      return undefined;
    }
    let active = true;
    let stop: (() => void) | undefined;
    void listen<unknown>("backup://progress", (event) => {
      const payload = eventPayload(event.payload);
      if (!active || payload === null) {
        return;
      }
      const operation = payload["operation"];
      const phase = payload["phase"];
      const permille = payload["permille"];
      if (
        (operation !== "backup" && operation !== "restore") ||
        typeof phase !== "string" ||
        typeof permille !== "number"
      ) {
        return;
      }
      // Only the section re-renders, and only at a phase boundary: the copy itself runs in
      // Rust and never asks the webview to paint a frame.
      setState((previous) => ({ ...previous, progress: { operation, phase, permille } }));
    })
      .then((unlisten) => {
        if (active) {
          stop = unlisten;
        } else {
          unlisten();
        }
      })
      .catch((caught: unknown) => {
        logUiError("listenBackupProgress", normalizeIpcError(caught));
      });
    return () => {
      active = false;
      stop?.();
    };
  }, []);

  const create = useCallback(async (): Promise<boolean> => {
    const picked = await pickFolder();
    if (picked === null || picked.kind !== "folder") {
      return false;
    }
    setState((previous) => ({
      ...previous,
      busy: true,
      error: null,
      active: "backup",
      progress: null,
    }));
    try {
      const backup = await createBackup(picked.path);
      setState({
        busy: false,
        outcome: { kind: "created", backup },
        error: null,
        active: null,
        progress: null,
      });
      return true;
    } catch (caught: unknown) {
      const error = normalizeIpcError(caught);
      logUiError("createBackup", error);
      setState((previous) => ({
        ...previous,
        busy: false,
        error: messageOf(error),
        active: null,
        progress: null,
      }));
      return false;
    }
  }, []);

  const restore = useCallback(async (source: string): Promise<boolean> => {
    setState((previous) => ({
      ...previous,
      busy: true,
      error: null,
      active: "restore",
      progress: null,
    }));
    try {
      const answer = await restoreBackup(source);
      // The process is restarting. Leave the section busy and say so; the promise may never
      // settle if the exit wins the race, which is a success we cannot observe from here.
      setState((previous) => ({
        busy: true,
        outcome: { kind: "restoring", restore: answer },
        error: null,
        active: "restore",
        progress: previous.progress,
      }));
      return true;
    } catch (caught: unknown) {
      const error = normalizeIpcError(caught);
      logUiError("restoreBackup", error);
      setState((previous) => ({
        ...previous,
        busy: false,
        error: messageOf(error),
        active: null,
        progress: null,
      }));
      return false;
    }
  }, []);

  return { ...state, create, restore };
}
