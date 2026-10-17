/**
 * Reads whether this installation has ever been set up (PG-E26-11).
 *
 * The two reads are the existing commands rather than a new flag: `list_roots` is the list the
 * library screen already draws, and `status` is the provider answer the settings screen already
 * reads. A dedicated "has been set up" bit would be a third source of the same fact and one that
 * could disagree with the two the app actually uses.
 *
 * The read happens once, on mount. It must not re-read after the wizard adds a root or stores a
 * key: the gate is meant to stay on the wizard until Done, and a live check would swap the user
 * into the shell mid-step — which is exactly the moment they have not finished.
 */

import {
  commands,
  listLibraryRoots,
  normalizeIpcError,
  unwrap,
  type LibraryRootDto,
  type ProviderStatusDto,
} from "@peregrine/ipc-client";
import { useEffect, useState } from "react";
import { logUiError } from "../../../lib/error-log.js";
import { isFirstRun, type FirstRunCheck } from "../first-run.js";

/** The first-run check, asked once per mount. */
export function useFirstRun(): FirstRunCheck {
  const [check, setCheck] = useState<FirstRunCheck>({ state: "checking" });

  useEffect(() => {
    let cancelled = false;

    void (async () => {
      try {
        const [roots, providers]: [LibraryRootDto[], ProviderStatusDto] = await Promise.all([
          listLibraryRoots(),
          unwrap(commands.status()),
        ]);
        if (cancelled) {
          return;
        }
        setCheck(isFirstRun(roots, providers) ? { state: "first-run" } : { state: "configured" });
      } catch (caught: unknown) {
        const error = normalizeIpcError(caught);
        logUiError("setup.firstRun", error);
        if (!cancelled) {
          setCheck({ state: "unknown" });
        }
      }
    })();

    return () => {
      cancelled = true;
    };
  }, []);

  return check;
}
