import {
  getBuildInfo,
  getProviderAttribution,
  normalizeIpcError,
  type AttributionDto,
  type BuildInfo,
} from "@peregrine/ipc-client";
import { useEffect, useState } from "react";

import { logUiError } from "../../../lib/error-log.js";

/** What the About section draws. */
export interface AboutInfo {
  /** The build's name, version, and target, or `null` before the read lands. */
  readonly build: BuildInfo | null;
  /** The notices this build is required to reproduce. */
  readonly attribution: readonly AttributionDto[];
  /** The read failed, so the section says so rather than drawing an empty credit. */
  readonly failed: boolean;
}

/**
 * Reads the two halves of About once.
 *
 * Both are reads with no runtime behind them (`PG-E5-19`), so an installation with no provider
 * configured still shows the notice it owes. A failed read is reported rather than drawn as "no
 * providers", which is a different and false statement.
 */
export function useAbout(): AboutInfo {
  const [state, setState] = useState<AboutInfo>({
    build: null,
    attribution: [],
    failed: false,
  });

  useEffect(() => {
    let cancelled = false;
    void Promise.all([getBuildInfo(), getProviderAttribution()])
      .then(([build, attribution]) => {
        if (!cancelled) {
          setState({ build, attribution, failed: false });
        }
      })
      .catch((caught: unknown) => {
        logUiError("about", normalizeIpcError(caught));
        if (!cancelled) {
          setState({ build: null, attribution: [], failed: true });
        }
      });
    return () => {
      cancelled = true;
    };
  }, []);

  return state;
}
