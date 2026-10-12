/**
 * The hook layer for the library roots: which folders the core is allowed to scan, and the two
 * things a user does about them.
 *
 * Three states are distinguished rather than collapsed, for the reason `useShortcuts` gives: an
 * unreachable core answers nothing, and a list reading "you have no libraries" would be telling the
 * user something the app does not know. A failed read says so, and the rows are unknown rather than
 * empty.
 *
 * The writes are optimistic in no direction at all: each one awaits the core and then re-reads, so
 * what the screen shows is what the database holds. A folder that is already a root, or a path the
 * core refuses, comes back as an error the caller shows rather than as a row that quietly
 * disappears.
 */

import {
  addLibraryRoot,
  describeIpcError,
  listLibraryRoots,
  normalizeIpcError,
  removeLibraryRoot,
} from "@peregrine/ipc-client";
import type { IpcError, LibraryRootDto, RootKind } from "@peregrine/ipc-client";
import { useCallback, useEffect, useState } from "react";
import { logUiError } from "../../../lib/error-log.js";
import { pickFolder } from "../../../lib/picker.js";

/** The roots as the core holds them, and what the screen needs to know about the read. */
export interface LibraryRoots {
  /** The roots, newest first; empty until the read answers. */
  readonly roots: readonly LibraryRootDto[];
  /** The read is still in flight. */
  readonly loading: boolean;
  /** The read failed, so the list is unknown rather than empty. */
  readonly failed: boolean;
  /** A write is in flight, so the buttons that write are disabled. */
  readonly busy: boolean;
  /** What the last write failed with, for the screen to show beside the list. */
  readonly error: string | null;
  /**
   * Opens the folder picker and adds what the user chose.
   *
   * Answers with the added root, or `null` when the user cancelled. Resolves rather than throwing,
   * because the caller is a click handler: a refusal — a folder that is already a library, a path
   * the core will not take — is a fact about the attempt, not an exception.
   */
  readonly add: (kind: RootKind) => Promise<LibraryRootDto | null>;
  /** Removes one root, answering whether the core had it. */
  readonly remove: (id: number) => Promise<boolean>;
}

/**
 * What to show for a failed write.
 *
 * The core's own detail is a sentence written for a user — "That folder is already one of your
 * libraries." — so it wins over the code's generic copy. `describeIpcError` answers with keys
 * rather than text, which is why this is the message and not the copy.
 */
function messageOf(error: IpcError): string {
  return error.detail ?? error.message ?? describeIpcError(error).titleKey;
}

/** The scan policy a folder added from Settings gets. */
const ADDED_SCAN_MODE = "incremental" as const;

/**
 * The library roots, and the operations the settings screen offers.
 *
 * `add` opens the picker itself rather than taking a path, because the picker is the only way this
 * screen produces a path and a component that took one would have to know how to get it. It uses
 * the shell's own `pickFolder`, which refuses outside the Tauri shell — so this hook is testable in
 * a browser context only for its reads.
 */
export function useLibraryRoots(): LibraryRoots {
  const [state, setState] = useState<{
    roots: readonly LibraryRootDto[];
    loading: boolean;
    failed: boolean;
    busy: boolean;
    error: string | null;
  }>({ roots: [], loading: true, failed: false, busy: false, error: null });

  const read = useCallback(async (): Promise<void> => {
    try {
      const roots = await listLibraryRoots();
      setState((previous) => ({ ...previous, roots, loading: false, failed: false }));
    } catch (caught: unknown) {
      const error = normalizeIpcError(caught);
      logUiError("listLibraryRoots", error);
      setState((previous) => ({ ...previous, roots: [], loading: false, failed: true }));
    }
  }, []);

  useEffect(() => {
    void read();
  }, [read]);

  const add = useCallback(
    async (kind: RootKind): Promise<LibraryRootDto | null> => {
      const picked = await pickFolder();
      // `null` is the shell answering that there is no picker here at all; `cancelled` is the user
      // closing it. Both mean nothing to add, and neither is an error to report.
      if (picked === null || picked.kind !== "folder") {
        return null;
      }
      setState((previous) => ({ ...previous, busy: true, error: null }));
      try {
        const added = await addLibraryRoot(picked.path, kind, ADDED_SCAN_MODE);
        await read();
        return added;
      } catch (caught: unknown) {
        const error = normalizeIpcError(caught);
        logUiError("addLibraryRoot", error);
        setState((previous) => ({ ...previous, error: messageOf(error) }));
        return null;
      } finally {
        setState((previous) => ({ ...previous, busy: false }));
      }
    },
    [read],
  );

  const remove = useCallback(
    async (id: number): Promise<boolean> => {
      setState((previous) => ({ ...previous, busy: true, error: null }));
      try {
        const removed = await removeLibraryRoot(id);
        await read();
        return removed;
      } catch (caught: unknown) {
        const error = normalizeIpcError(caught);
        logUiError("removeLibraryRoot", error);
        setState((previous) => ({ ...previous, error: messageOf(error) }));
        return false;
      } finally {
        setState((previous) => ({ ...previous, busy: false }));
      }
    },
    [read],
  );

  return { ...state, add, remove };
}
