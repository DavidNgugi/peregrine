/**
 * The hook layer for one title's own screen.
 *
 * Two reads, because the screen shows two things: the title with its totals, and its episodes in the
 * order a person watches them. They are separate because they are separate questions — a film has no
 * episodes, and a title the library holds nothing of still has a page.
 *
 * **`missing` is its own state rather than an error.** `getTitle` answers `null` for a title this
 * account may not see and for an id that names nothing, and the two are the same answer by design
 * (I37). A screen that showed a failure for either would be telling the user something the core did
 * not say.
 */

import { getTitle, listEpisodes, normalizeIpcError } from "@peregrine/ipc-client";
import type { EpisodeDto, IpcError, LibraryTitleDto } from "@peregrine/ipc-client";
import { useCallback, useEffect, useState } from "react";
import { logUiError } from "../../../lib/error-log.js";

/** One title, its episodes, and what the screen needs to know about the reads. */
export interface TitleDetail {
  /** The title, once the read has answered. */
  readonly title: LibraryTitleDto | null;
  /** Its episodes, in watch order; empty for a film and for a title with nothing here. */
  readonly episodes: readonly EpisodeDto[];
  /** The reads are still in flight. */
  readonly loading: boolean;
  /** The core answered that there is no such title here. */
  readonly missing: boolean;
  /** A read failed, so what is here is unknown rather than absent. */
  readonly failed: boolean;
  /** Re-reads the title and its episodes. */
  readonly refresh: () => Promise<void>;
}

/**
 * One title and its episodes.
 *
 * `null` is "no title route is open", which is a state the shell is in on every other screen. The
 * hook cannot be called conditionally — the rules of hooks — so the *read* is what is conditional: a
 * hook that asked the core about id zero on every route would be a request per navigation whose
 * answer nothing renders.
 */
export function useTitle(id: number | null): TitleDetail {
  const [state, setState] = useState<Omit<TitleDetail, "refresh">>({
    title: null,
    episodes: [],
    loading: true,
    missing: false,
    failed: false,
  });

  const read = useCallback(async (): Promise<void> => {
    if (id === null) {
      // Nothing to read and nothing to say: the screen that would show it is not rendered. Loading
      // rather than missing, because "this title is not here" is an answer and this is not one.
      setState({
        title: null,
        episodes: [],
        loading: true,
        missing: false,
        failed: false,
      });
      return;
    }
    try {
      const title = await getTitle(id);
      if (title === null) {
        setState({
          title: null,
          episodes: [],
          loading: false,
          missing: true,
          failed: false,
        });
        return;
      }
      // Only asked once the title is known: a film has no episodes, and asking the core to prove that
      // costs a query whose answer the title's kind already gives.
      const episodes = title.kind === "movies" ? [] : await listEpisodes(id);
      setState({ title, episodes, loading: false, missing: false, failed: false });
    } catch (caught: unknown) {
      const error: IpcError = normalizeIpcError(caught);
      logUiError("getTitle", error);
      setState({
        title: null,
        episodes: [],
        loading: false,
        missing: false,
        failed: true,
      });
    }
  }, [id]);

  useEffect(() => {
    void read();
  }, [read]);

  return { ...state, refresh: read };
}
