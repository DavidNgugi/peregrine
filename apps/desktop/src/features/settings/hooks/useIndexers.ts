/**
 * The Prowlarr configuration a settings screen drives, and the indexers it holds.
 *
 * These commands are synchronous, unlike a provider key: storing one proves the key against the
 * provider with a job, while Prowlarr answers a health check in one request and the person is
 * already waiting on this screen. Nothing here computes whether an indexer is usable — the core
 * answers that, and this hook renders the answer.
 *
 * The API key is written and never read back. `configured` is a flag and the core returns the base
 * URL alone, so the only key this hook ever holds is the candidate in the field.
 *
 * The catalog is a summary list on purpose: every definition carries dozens of fields and reading
 * them all for one IPC answer would blow the payload budget (AGENTS R11), so the fields are read
 * only for the definition the user picks.
 */

import {
  addIndexer,
  clearIndexer,
  indexerCatalog,
  indexerDefinition,
  indexerStatus,
  listIndexers,
  normalizeIpcError,
  removeIndexer,
  setIndexer,
  testIndexer,
  type AddIndexerRequest,
  type ConfiguredIndexerDto,
  type IndexerDefinitionDto,
  type IndexerStatusDto,
  type IndexerSummaryDto,
} from "@peregrine/ipc-client";
import { useCallback, useEffect, useState } from "react";
import { logUiError } from "../../../lib/error-log.js";
import { messageOf } from "../../../lib/ipc-message.js";

/** What the last completed action left behind, for the sentence under the controls. */
export type IndexerOutcome =
  "saved" | "tested" | "testFailed" | "removed" | "added" | "deleted" | null;

/** The configured indexer, the indexers Prowlarr holds, and the operations the section offers. */
export interface IndexerSettings {
  /** The core's answer, or `null` before it has answered. */
  readonly status: IndexerStatusDto | null;
  /** The indexers Prowlarr currently holds. */
  readonly indexers: ConfiguredIndexerDto[];
  /** The addable definitions, without their fields. */
  readonly catalog: IndexerSummaryDto[];
  /** The fields of the definition the user picked. */
  readonly definition: IndexerDefinitionDto | null;
  /** The first read is still in flight. */
  readonly loading: boolean;
  /** The catalog is being loaded, so the browse control can say so. */
  readonly catalogLoading: boolean;
  /** The read failed, so the answer is unknown rather than "not configured". */
  readonly failed: boolean;
  /** An action is in flight, so the controls are refused. */
  readonly busy: boolean;
  /** What the last action did, or `null` before one finishes. */
  readonly outcome: IndexerOutcome;
  /** What the last action failed with, in the core's own words. */
  readonly error: string | null;
  /** Stores a candidate base URL and key. */
  readonly save: (baseUrl: string, apiKey: string) => Promise<void>;
  /** Proves a candidate base URL and key answer. */
  readonly test: (baseUrl: string, apiKey: string) => Promise<void>;
  /** Removes the stored configuration. */
  readonly clear: () => Promise<void>;
  /** Loads the addable definitions. */
  readonly loadCatalog: () => Promise<void>;
  /** Loads one definition's fields. */
  readonly choose: (key: string) => Promise<void>;
  /** Adds one indexer. */
  readonly add: (request: AddIndexerRequest) => Promise<void>;
  /** Removes one indexer by Prowlarr's id. */
  readonly remove: (id: number) => Promise<void>;
  /** Re-reads the status and the configured list. */
  readonly refresh: () => Promise<void>;
}

/** The configured indexer, and the things a person does about it. */
export function useIndexers(): IndexerSettings {
  const [status, setStatus] = useState<IndexerStatusDto | null>(null);
  const [rows, setRows] = useState<ConfiguredIndexerDto[]>([]);
  const [catalog, setCatalog] = useState<IndexerSummaryDto[]>([]);
  const [definition, setDefinition] = useState<IndexerDefinitionDto | null>(null);
  const [loading, setLoading] = useState(true);
  const [catalogLoading, setCatalogLoading] = useState(false);
  const [failed, setFailed] = useState(false);
  const [busy, setBusy] = useState(false);
  const [outcome, setOutcome] = useState<IndexerOutcome>(null);
  const [error, setError] = useState<string | null>(null);

  const read = useCallback(async (): Promise<void> => {
    try {
      const answer = await indexerStatus();
      setStatus(answer);
      setFailed(false);
      setRows(answer.configured ? await listIndexers() : []);
    } catch (caught: unknown) {
      const normalized = normalizeIpcError(caught);
      logUiError("indexers.status", normalized);
      setFailed(true);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void read();
  }, [read]);

  // One action at a time: the controls are disabled while `busy`, and the read after each write is
  // what makes the "configured" line the core's answer rather than the form's assumption.
  const action = useCallback(
    async (call: () => Promise<void>, done: IndexerOutcome, tag: string): Promise<void> => {
      setBusy(true);
      setError(null);
      setOutcome(null);
      try {
        await call();
        setOutcome(done);
      } catch (caught: unknown) {
        const normalized = normalizeIpcError(caught);
        logUiError(tag, normalized);
        setError(messageOf(normalized));
      } finally {
        setBusy(false);
        await read();
      }
    },
    [read],
  );

  const save = useCallback(
    (baseUrl: string, apiKey: string) =>
      action(
        async () => {
          setStatus(await setIndexer(baseUrl, apiKey));
        },
        "saved",
        "indexers.set",
      ),
    [action],
  );

  const test = useCallback(async (baseUrl: string, apiKey: string): Promise<void> => {
    setBusy(true);
    setError(null);
    setOutcome(null);
    try {
      const answered = await testIndexer(baseUrl, apiKey);
      setOutcome(answered ? "tested" : "testFailed");
    } catch (caught: unknown) {
      const normalized = normalizeIpcError(caught);
      logUiError("indexers.test", normalized);
      setError(messageOf(normalized));
    } finally {
      setBusy(false);
    }
  }, []);

  const clear = useCallback(
    () =>
      action(
        async () => {
          setStatus(await clearIndexer());
        },
        "removed",
        "indexers.clear",
      ),
    [action],
  );

  const loadCatalog = useCallback(async (): Promise<void> => {
    setError(null);
    setCatalogLoading(true);
    try {
      setCatalog(await indexerCatalog());
    } catch (caught: unknown) {
      const normalized = normalizeIpcError(caught);
      logUiError("indexers.catalog", normalized);
      setError(messageOf(normalized));
    } finally {
      setCatalogLoading(false);
    }
  }, []);

  const choose = useCallback(async (key: string): Promise<void> => {
    if (key.length === 0) {
      setDefinition(null);
      return;
    }
    try {
      setDefinition(await indexerDefinition(key));
    } catch (caught: unknown) {
      const normalized = normalizeIpcError(caught);
      logUiError("indexers.definition", normalized);
      setError(messageOf(normalized));
    }
  }, []);

  const add = useCallback(
    (request: AddIndexerRequest) =>
      action(
        async () => {
          setRows(await addIndexer(request));
          setDefinition(null);
        },
        "added",
        "indexers.add",
      ),
    [action],
  );

  const remove = useCallback(
    (id: number) =>
      action(
        async () => {
          setRows(await removeIndexer({ id }));
        },
        "deleted",
        "indexers.remove",
      ),
    [action],
  );

  return {
    status,
    indexers: rows,
    catalog,
    definition,
    loading,
    catalogLoading,
    failed,
    busy,
    outcome,
    error,
    save,
    test,
    clear,
    loadCatalog,
    choose,
    add,
    remove,
    refresh: read,
  };
}
