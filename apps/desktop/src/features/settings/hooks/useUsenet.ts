/**
 * The Usenet provider configuration a settings screen drives.
 *
 * A password is written and never read back. The status carries the provider rows and their health;
 * the form holds only the password the person is typing.
 */

import {
  commands,
  normalizeIpcError,
  type UsenetServerInputDto,
  type UsenetStatusDto,
} from "@peregrine/ipc-client";
import { useCallback, useEffect, useState } from "react";
import { messageOf } from "../../../lib/ipc-message.js";

/** The configured providers, the archive policy, and the operations the section offers. */
export interface UsenetSettings {
  readonly status: UsenetStatusDto | null;
  readonly loading: boolean;
  readonly failed: boolean;
  readonly busy: boolean;
  readonly error: string | null;
  readonly save: (input: UsenetServerInputDto) => Promise<void>;
  readonly remove: (id: number) => Promise<void>;
  readonly test: (input: UsenetServerInputDto) => Promise<void>;
  readonly setArchives: (allow: boolean, remove: boolean) => Promise<void>;
  readonly refresh: () => Promise<void>;
}

/** The configured providers, and the things a person does about them. */
export function useUsenet(): UsenetSettings {
  const [status, setStatus] = useState<UsenetStatusDto | null>(null);
  const [loading, setLoading] = useState(true);
  const [failed, setFailed] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    const result = await commands.usenetStatus();
    if (result.status === "ok") {
      setStatus(result.data);
      setFailed(false);
    } else {
      setFailed(true);
      setError(messageOf(normalizeIpcError(result.error)));
    }
    setLoading(false);
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const act = useCallback(
    async (operation: () => Promise<{ status: "ok" | "error"; data?: UsenetStatusDto; error?: unknown }>) => {
      setBusy(true);
      setError(null);
      try {
        const result = await operation();
        if (result.status === "ok" && result.data !== undefined) {
          setStatus(result.data);
          setFailed(false);
        } else if (result.status === "error") {
          setError(messageOf(normalizeIpcError(result.error)));
        }
      } finally {
        setBusy(false);
      }
    },
    [],
  );

  const save = useCallback(
    async (input: UsenetServerInputDto) => {
      await act(async () => commands.saveUsenetServer(input));
    },
    [act],
  );

  const remove = useCallback(
    async (id: number) => {
      await act(async () =>
        commands.removeUsenetServer({
          id,
          name: "",
          host: "",
          port: 563,
          tls: true,
          username: "",
          password: null,
          connections: 8,
          priority: 100,
          enabled: false,
        }),
      );
    },
    [act],
  );

  const test = useCallback(
    async (input: UsenetServerInputDto) => {
      const result = await commands.testUsenetServer(input);
      if (result.status === "error") {
        setError(messageOf(normalizeIpcError(result.error)));
      }
      await refresh();
    },
    [refresh],
  );

  const setArchives = useCallback(
    async (allow: boolean, remove: boolean) => {
      await act(async () => commands.setUsenetArchives(allow, remove));
    },
    [act],
  );

  return { status, loading, failed, busy, error, save, remove, test, setArchives, refresh };
}
