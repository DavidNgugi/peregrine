import { listShortcuts, normalizeIpcError } from "@peregrine/ipc-client";
import type { ShortcutBinding } from "@peregrine/ipc-client";
import { useEffect, useState } from "react";
import { logUiError } from "../../../lib/error-log.js";
import { shortcutFor } from "../shortcut-dispatch.js";

/**
 * The shortcut registry as the core declares it.
 *
 * An unreachable core answers with the empty list rather than throwing: the shell still
 * has a window, a menu, and a tray, and a keyboard registry that cannot be read must not
 * stop it from starting (docs/ARCHITECTURE.md §15).
 */
export function useShortcuts(): readonly ShortcutBinding[] {
  const [bindings, setBindings] = useState<readonly ShortcutBinding[]>([]);

  useEffect(() => {
    let cancelled = false;
    listShortcuts()
      .then((loaded) => {
        if (!cancelled) {
          setBindings(loaded);
        }
      })
      .catch((caught: unknown) => {
        logUiError("listShortcuts", normalizeIpcError(caught));
      });
    return () => {
      cancelled = true;
    };
  }, []);

  return bindings;
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
