import type { IpcError } from "@peregrine/ipc-client";

/**
 * One structured line per UI error.
 *
 * `detail` and `cause` are left out on purpose: they can carry provider text or a
 * path, and the redacting log sink is PG-E25-01. The message the core sent is
 * already scrubbed at the IPC boundary (docs/ARCHITECTURE.md §7 rule 3).
 */
export function logUiError(context: string, error: IpcError): void {
  console.error(
    JSON.stringify({
      event: "ui.error",
      context,
      code: error.code,
      message: error.message,
      retryable: error.retryable,
    }),
  );
}
