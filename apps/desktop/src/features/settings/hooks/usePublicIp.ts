/** The public-address check a reader runs after starting a tunnel: one request, on demand. */

import { normalizeIpcError, publicIp } from "@peregrine/ipc-client";
import { useCallback, useState } from "react";

import { logUiError } from "../../../lib/error-log.js";

export interface PublicIpCheck {
  /** The last address the echo service answered, or `null` before one did. */
  readonly ip: string | null;
  readonly checking: boolean;
  readonly failed: boolean;
  /** Asks once. The answer replaces the previous one; a refusal never invents an address. */
  readonly check: () => Promise<void>;
}

export function usePublicIp(): PublicIpCheck {
  const [state, setState] = useState<Omit<PublicIpCheck, "check">>({
    ip: null,
    checking: false,
    failed: false,
  });

  const check = useCallback(async (): Promise<void> => {
    setState((previous) => ({ ...previous, checking: true, failed: false }));
    try {
      const ip = await publicIp();
      setState({ ip, checking: false, failed: false });
    } catch (caught: unknown) {
      logUiError("publicIp", normalizeIpcError(caught));
      setState({ ip: null, checking: false, failed: true });
    }
  }, []);

  return { ...state, check };
}
