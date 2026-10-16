/**
 * The quality downloads prefer, and the one control that changes it.
 *
 * The core stores the choice as the default profile's resolution list, which is the ranker's
 * accept list: naming 2160p is what makes a 4K release a candidate, and leaving it out is what
 * keeps one out. A list no choice names — a hand-edited profile — reads back as null, and the
 * screen shows that rather than relabelling it.
 */

import { getPreferredQuality, normalizeIpcError, setPreferredQuality } from "@peregrine/ipc-client";
import { useCallback, useEffect, useState } from "react";
import { logUiError } from "../../../lib/error-log.js";

/** The choices the setting offers, in the order the wire spells them. */
export const PREFERRED_QUALITIES = ["1080p", "1440p", "2160p", "best"] as const;

/** One of those choices. */
export type PreferredQuality = (typeof PREFERRED_QUALITIES)[number];

/** The preferred quality, as the Settings row reads and changes it. */
export interface PreferredQualityControl {
  /** The stored choice, or null when the profile's list is not one of the choices. */
  readonly quality: PreferredQuality | null;
  /** A read or write is in flight. */
  readonly busy: boolean;
  /** The last read or write failed. */
  readonly failed: boolean;
  /** Stores a new choice. */
  readonly choose: (next: PreferredQuality) => void;
}

/** The preferred quality and the action that changes it. */
export function usePreferredQuality(): PreferredQualityControl {
  const [state, setState] = useState<Omit<PreferredQualityControl, "choose">>({
    quality: null,
    busy: true,
    failed: false,
  });

  useEffect(() => {
    let cancelled = false;
    getPreferredQuality()
      .then((value) => {
        if (!cancelled) {
          setState({ quality: asChoice(value), busy: false, failed: false });
        }
      })
      .catch((caught: unknown) => {
        logUiError("preferredQuality", normalizeIpcError(caught));
        if (!cancelled) {
          setState({ quality: null, busy: false, failed: true });
        }
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const choose = useCallback((next: PreferredQuality): void => {
    setState((previous) => ({ ...previous, busy: true }));
    setPreferredQuality(next)
      .then(() => {
        setState({ quality: next, busy: false, failed: false });
      })
      .catch((caught: unknown) => {
        logUiError("setPreferredQuality", normalizeIpcError(caught));
        setState((previous) => ({ ...previous, busy: false, failed: true }));
      });
  }, []);

  return { ...state, choose };
}

function asChoice(value: string | null): PreferredQuality | null {
  return PREFERRED_QUALITIES.find((choice) => choice === value) ?? null;
}
