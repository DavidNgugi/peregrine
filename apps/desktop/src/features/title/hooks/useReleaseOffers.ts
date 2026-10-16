/**
 * The releases a search left at qualities the preferred profile excluded.
 *
 * The list is the core's: a search that finds nothing at the preferred quality persists what it did
 * find, and this reads them back so the page can offer a choice rather than a dead end. Nothing here
 * ranks or filters — the core already ordered them, and each release's source stays server-side, so
 * this hook holds ids and metadata only (R11).
 */

import {
  chooseReleaseOffer,
  dismissReleaseOffer,
  listReleaseOffers,
  normalizeIpcError,
  type ReleaseOfferDto,
} from "@peregrine/ipc-client";
import type { IpcError } from "@peregrine/ipc-client";
import { useCallback, useEffect, useState } from "react";
import { logUiError } from "../../../lib/error-log.js";

/** What the title page's alternatives picker reads. */
export interface ReleaseOffers {
  /** The alternatives the core kept, best first. */
  readonly offers: ReleaseOfferDto[];
  /** The read is in flight. */
  readonly loading: boolean;
  /** The read failed, so the list is unknown rather than empty. */
  readonly failed: boolean;
  /** The offer a choice or a dismissal is in flight for. */
  readonly busyId: number | null;
  /** A choice was accepted, so the release is being found and queued. */
  readonly started: boolean;
  /** What the last action failed with, in the core's own words. */
  readonly error: IpcError | null;
  /** Queues the chosen release. */
  readonly choose: (offerId: number) => Promise<void>;
  /** Dismisses one offer. */
  readonly dismiss: (offerId: number) => Promise<void>;
  /** Re-reads the list. */
  readonly refresh: () => Promise<void>;
}

/**
 * The alternatives for one title.
 *
 * `reloadKey` is what the caller changes when something upstream may have produced offers — the
 * acquire job settling, in practice — because the persistence happens inside that job and a read on
 * mount alone would predate it.
 */
export function useReleaseOffers(titleId: number, reloadKey: string | null): ReleaseOffers {
  const [offers, setOffers] = useState<ReleaseOfferDto[]>([]);
  const [loading, setLoading] = useState(true);
  const [failed, setFailed] = useState(false);
  const [busyId, setBusyId] = useState<number | null>(null);
  const [started, setStarted] = useState(false);
  const [error, setError] = useState<IpcError | null>(null);

  const read = useCallback(async (): Promise<void> => {
    setLoading(true);
    try {
      setOffers(await listReleaseOffers(titleId));
      setFailed(false);
    } catch (caught: unknown) {
      const normalized = normalizeIpcError(caught);
      logUiError("releaseOffers.list", normalized);
      setFailed(true);
    } finally {
      setLoading(false);
    }
  }, [titleId]);

  useEffect(() => {
    void read();
  }, [read, reloadKey]);

  const choose = useCallback(
    async (offerId: number): Promise<void> => {
      setBusyId(offerId);
      setStarted(false);
      setError(null);
      try {
        await chooseReleaseOffer(offerId);
        setStarted(true);
        await read();
      } catch (caught: unknown) {
        const normalized = normalizeIpcError(caught);
        logUiError("releaseOffers.choose", normalized);
        setError(normalized);
      } finally {
        setBusyId(null);
      }
    },
    [read],
  );

  const dismiss = useCallback(
    async (offerId: number): Promise<void> => {
      setBusyId(offerId);
      setError(null);
      try {
        await dismissReleaseOffer(offerId);
        await read();
      } catch (caught: unknown) {
        const normalized = normalizeIpcError(caught);
        logUiError("releaseOffers.dismiss", normalized);
        setError(normalized);
      } finally {
        setBusyId(null);
      }
    },
    [read],
  );

  return { offers, loading, failed, busyId, started, error, choose, dismiss, refresh: read };
}
