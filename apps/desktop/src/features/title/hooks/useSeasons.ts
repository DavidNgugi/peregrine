/**
 * The seasons of one title, and the episodes of the season on screen.
 *
 * **One hook rather than two, because the second read depends on the first.** The seasons decide which
 * season opens (`seasons.ts`'s `defaultSeason`), and the episodes are that season's — a hook that read
 * them separately would have to draw an empty list until it learned which one to ask for, which is the
 * flicker the route's kind already exists to avoid.
 *
 * **The read is per season rather than per title.** The first shape read every episode of the title and
 * filtered in the screen, which is three reads' worth of rows to draw one season and a rule living in
 * TypeScript that the core can express as a predicate. `list_episodes` now takes the season, and the
 * screen asks for the one it is showing.
 *
 * **Choosing a season re-reads rather than filtering.** What a season holds is the core's answer, and a
 * screen that kept every season's episodes in memory to switch between them would be holding a second
 * copy of the catalogue — the copy that drifts from the first.
 *
 * **The read follows the refresh, because a first visit has no seasons to read yet.** A show nobody has
 * filled in is opened before its refresh job has written a single season, so the read that runs on the
 * way in answers nothing; `refreshes` moves when that job stops, and both reads run again. Without it
 * the page stayed empty until the reader left and came back — the "it never loads" state this hook
 * was the wrong half of.
 *
 * **A re-read keeps the season on screen** (`seasons.ts`'s `seasonAfterRead`): moving a reader from
 * season four back to season one because a poster arrived is a screen that undoes their choice.
 */

import { listEpisodes, listSeasons, normalizeIpcError } from "@peregrine/ipc-client";
import type { EpisodeDto, IpcError, SeasonDto } from "@peregrine/ipc-client";
import { useCallback, useEffect, useRef, useState } from "react";

import { logUiError } from "../../../lib/error-log.js";
import type { TitleKind } from "../../../app/routes.js";
import { seasonAfterRead } from "./seasons.js";

/** What a title's season section is doing. */
export interface TitleSeasons {
  /** Every season the read answered, in number order. */
  readonly seasons: readonly SeasonDto[];
  /** The season on screen, or `null` while the read is in the air. */
  readonly season: SeasonDto | null;
  /** That season's episodes, in the order they are watched. */
  readonly episodes: readonly EpisodeDto[];
  /** A read is in flight. */
  readonly loading: boolean;
  /** A read failed, so what is here is short rather than complete. */
  readonly failed: IpcError | null;
  /** Shows another season, re-reading its episodes. */
  readonly choose: (seasonNumber: number) => void;
}

/**
 * The seasons of `titleId`, and the episodes of the one on screen.
 *
 * A film asks for nothing: `kind` short-circuits both reads, because a film has no seasons and asking
 * the core to prove that is a request spent on a question with one answer.
 *
 * `refreshes` is how many title refreshes this screen watched stop. It is a trigger and not a value:
 * the reads below run again when it moves, so a refresh that wrote the seasons is drawn without
 * leaving the page.
 */
export function useSeasons(
  titleId: number | null,
  kind: TitleKind,
  refreshes: number,
): TitleSeasons {
  const [seasons, setSeasons] = useState<readonly SeasonDto[]>([]);
  const [chosen, setChosen] = useState<number | null>(null);
  const [episodes, setEpisodes] = useState<readonly EpisodeDto[]>([]);
  const [state, setState] = useState<{
    loading: boolean;
    failed: IpcError | null;
  }>({ loading: false, failed: null });
  // Which title, and which season, the answers belong to: a read that lands after the reader moved on
  // is dropped rather than drawn under the wrong heading.
  const version = useRef(0);
  // What the state above is *about*. A re-read of the same title keeps its season and its list; a
  // move to another title clears both, so one title's season four cannot be drawn under the next
  // title's heading.
  const reading = useRef<string | null>(null);

  // The season list, once per title and again whenever a refresh lands.
  useEffect(() => {
    if (titleId === null || kind === "movie") {
      version.current += 1;
      reading.current = null;
      setSeasons([]);
      setChosen(null);
      setEpisodes([]);
      setState({ loading: false, failed: null });
      return;
    }
    const visit = `${String(titleId)}:${kind}`;
    if (reading.current !== visit) {
      reading.current = visit;
      setSeasons([]);
      setChosen(null);
      setEpisodes([]);
    }
    const request = ++version.current;
    setState({ loading: true, failed: null });
    void (async () => {
      try {
        const found = await listSeasons(titleId);
        if (request !== version.current) {
          return;
        }
        setSeasons(found);
        // The opening season is the core's answer projected onto a display rule; `null` is a show whose
        // provider named no seasons, which draws no section rather than an empty one. A re-read keeps
        // the one on screen when it is still there.
        setChosen((current) => seasonAfterRead(found, current));
      } catch (caught: unknown) {
        if (request !== version.current) {
          return;
        }
        const error = normalizeIpcError(caught);
        logUiError("title.seasons", error);
        setState({ loading: false, failed: error });
      }
    })();
  }, [titleId, kind, refreshes]);

  // The chosen season's episodes.
  const load = useCallback(
    async (seasonNumber: number, mine: number) => {
      if (titleId === null) {
        return;
      }
      setState({ loading: true, failed: null });
      try {
        const found = await listEpisodes(titleId, seasonNumber);
        if (mine !== version.current) {
          return;
        }
        setEpisodes(found);
        setState({ loading: false, failed: null });
      } catch (caught: unknown) {
        if (mine !== version.current) {
          return;
        }
        const error = normalizeIpcError(caught);
        logUiError("title.episodes", error);
        // The episodes of the season that failed are cleared rather than left standing: what was on
        // screen belonged to another season, and leaving it under this heading would be the wrong list
        // with the right title on it.
        setEpisodes([]);
        setState({ loading: false, failed: error });
      }
    },
    [titleId],
  );

  // Re-read when the season changes and when a refresh lands: the season's episodes are written by
  // the same job the seasons are.
  useEffect(() => {
    if (chosen === null) {
      setEpisodes([]);
      return;
    }
    void load(chosen, version.current);
  }, [chosen, load, refreshes]);

  return {
    seasons,
    season: seasons.find((one) => one.seasonNumber === chosen) ?? null,
    episodes,
    loading: state.loading,
    failed: state.failed,
    choose: setChosen,
  };
}
