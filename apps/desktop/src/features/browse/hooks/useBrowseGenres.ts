/**
 * The genre vocabulary a Browse filter offers, for one kind.
 *
 * **Read once per kind per session, in the core.** The hook's part is only to ask and to hold the
 * answer: the cache that makes the second visit free lives in `Runtime::genres`, because a genre
 * vocabulary is provider knowledge and a webview that cached it would be a second copy of it.
 *
 * A failure is logged and drawn as an empty list rather than as an error beside the grid: a genre
 * control with nothing in it still browses, and replacing the page with a failure because one
 * dropdown could not fill would be telling the reader the catalogue is broken when the list is not.
 */

import { genres as listGenres, normalizeIpcError } from "@peregrine/ipc-client";
import type { GenreDto } from "@peregrine/ipc-client";
import { useEffect, useState } from "react";
import { logUiError } from "../../../lib/error-log.js";
import type { TitleKind } from "../../../app/routes.js";

/** The genres the provider filters `kind` by, in the order it lists them. */
export function useBrowseGenres(kind: TitleKind): readonly GenreDto[] {
  const [options, setOptions] = useState<readonly GenreDto[]>([]);

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const answered = await listGenres(kind);
        if (!cancelled) {
          setOptions(answered);
        }
      } catch (caught: unknown) {
        if (!cancelled) {
          logUiError("genres", normalizeIpcError(caught));
        }
      }
    })();

    return () => {
      cancelled = true;
    };
  }, [kind]);

  return options;
}
