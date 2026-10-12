/**
 * The hook layer for the shortcut list: one hook for the registry, and one for the keys
 * the shell acts on.
 *
 * The registry is the core's, so a failed read and an empty registry are different facts and
 * the screen has to be able to tell them apart: an unreachable core answers nothing, and a
 * list reading "no shortcuts are registered" would be telling the user something the app
 * does not know. `useAutoStart` reports `busy` and `failed` for the same reason.
 */

import { listShortcuts, normalizeIpcError } from "@peregrine/ipc-client";
import type { ShortcutBinding } from "@peregrine/ipc-client";
import { useEffect, useState } from "react";
import { logUiError } from "../../../lib/error-log.js";
import { shortcutFor } from "../shortcut-dispatch.js";

/** The core's shortcut registry, and whether reading it worked. */
export interface ShortcutRegistry {
  /** The registry as the core sent it; empty until the read answers. */
  readonly bindings: readonly ShortcutBinding[];
  /** The read is still in flight. */
  readonly loading: boolean;
  /** The read failed, so the list is unknown rather than empty. */
  readonly failed: boolean;
}

/**
 * The shortcut registry as the core declares it.
 *
 * An unreachable core answers with the empty list rather than throwing: the shell still has
 * a window, a menu, and a tray, and a keyboard registry that cannot be read must not stop it
 * from starting (docs/ARCHITECTURE.md §15). The failure is reported beside the list rather
 * than instead of the shell.
 */
export function useShortcuts(): ShortcutRegistry {
  const [state, setState] = useState<ShortcutRegistry>({
    bindings: [],
    loading: true,
    failed: false,
  });

  useEffect(() => {
    let cancelled = false;
    listShortcuts()
      .then((bindings) => {
        if (!cancelled) {
          setState({ bindings, loading: false, failed: false });
        }
      })
      .catch((caught: unknown) => {
        logUiError("listShortcuts", normalizeIpcError(caught));
        if (!cancelled) {
          setState({ bindings: [], loading: false, failed: true });
        }
      });
    return () => {
      cancelled = true;
    };
  }, []);

  return state;
}

/**
 * Runs `onAction` when the user presses a registered shortcut.
 *
 * The bindings are the registry the core sent, so a shortcut that is not in the Settings
 * list cannot fire and a listed one cannot be missing. `onAction` has to be stable — an
 * inline arrow would re-register the listener on every render — which is why the caller
 * wraps it in `useCallback`.
 */
export function useShortcutDispatch(
  bindings: readonly ShortcutBinding[],
  onAction: (id: string) => void,
): void {
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent): void => {
      const binding = shortcutFor(bindings, event);
      if (binding === null) {
        return;
      }
      // The webview and the application menu both see a key press; taking the default
      // away here is what keeps a registered shortcut from also typing into the page.
      event.preventDefault();
      onAction(binding.id);
    };
    window.addEventListener("keydown", onKeyDown);
    return () => {
      window.removeEventListener("keydown", onKeyDown);
    };
  }, [bindings, onAction]);
}
