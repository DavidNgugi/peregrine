/**
 * The error half of the IPC contract: the envelope of docs/ARCHITECTURE.md §7
 * rule 3 (`{ code, message, detail?, retryable, cause? }`) and the mapping from a
 * code to the message keys the UI renders.
 *
 * The vocabulary below is provisional. `xtask gen-ipc` (PG-E0-06) generates the
 * bindings from the Rust types and becomes the source of truth; until then this
 * module is hand-written, and `describeIpcError` is total on purpose: a code the
 * core adds before this list does still render a title and a next action, because
 * a blank error surface is worse than a generic one (docs/ARCHITECTURE.md §15).
 *
 * The wording itself lives in `@peregrine/i18n` (keys `ipc.error.<code>.<part>`),
 * so error copy is translated like every other string.
 */

export const IPC_ERROR_CODES = [
  "invalid_input",
  "not_found",
  "permission_denied",
  "unsafe_content",
  "disk_full",
  "rate_limited",
  "provider_unavailable",
  "database_busy",
  "internal",
] as const;

export type IpcErrorCode = (typeof IPC_ERROR_CODES)[number];

/** A structured error from the core, as it crosses the IPC boundary. */
export interface IpcError {
  readonly code: string;
  readonly message: string;
  readonly detail?: string;
  readonly cause?: string;
  readonly retryable: boolean;
}

/** Message keys for the copy that explains an error and what to do next (§1.2). */
export type IpcErrorMessageKey = `ipc.error.${IpcErrorCode}.${"title" | "advice"}`;

export interface IpcErrorCopy {
  readonly titleKey: IpcErrorMessageKey;
  readonly adviceKey: IpcErrorMessageKey;
}

export const IPC_ERROR_COPY: Readonly<Record<IpcErrorCode, IpcErrorCopy>> = {
  invalid_input: {
    titleKey: "ipc.error.invalid_input.title",
    adviceKey: "ipc.error.invalid_input.advice",
  },
  not_found: { titleKey: "ipc.error.not_found.title", adviceKey: "ipc.error.not_found.advice" },
  permission_denied: {
    titleKey: "ipc.error.permission_denied.title",
    adviceKey: "ipc.error.permission_denied.advice",
  },
  unsafe_content: {
    titleKey: "ipc.error.unsafe_content.title",
    adviceKey: "ipc.error.unsafe_content.advice",
  },
  disk_full: { titleKey: "ipc.error.disk_full.title", adviceKey: "ipc.error.disk_full.advice" },
  rate_limited: {
    titleKey: "ipc.error.rate_limited.title",
    adviceKey: "ipc.error.rate_limited.advice",
  },
  provider_unavailable: {
    titleKey: "ipc.error.provider_unavailable.title",
    adviceKey: "ipc.error.provider_unavailable.advice",
  },
  database_busy: {
    titleKey: "ipc.error.database_busy.title",
    adviceKey: "ipc.error.database_busy.advice",
  },
  internal: { titleKey: "ipc.error.internal.title", adviceKey: "ipc.error.internal.advice" },
};

/** True when `value` is one of the codes this build knows copy for. */
export function isIpcErrorCode(value: unknown): value is IpcErrorCode {
  return typeof value === "string" && IPC_ERROR_CODES.some((code) => code === value);
}

/** The copy keys for a structured error, falling back to the generic message. */
export function describeIpcError(error: IpcError): IpcErrorCopy {
  return isIpcErrorCode(error.code) ? IPC_ERROR_COPY[error.code] : IPC_ERROR_COPY.internal;
}

/**
 * Turns anything a call site can throw into a structured error.
 *
 * Nothing is stringified: an arbitrary thrown value could be a proxy, a cyclic
 * object, or a provider payload, and `detail`/`cause` are only kept when the core
 * sent strings, so foreign objects never reach the screen.
 */
export function normalizeIpcError(caught: unknown): IpcError {
  if (isRecord(caught)) {
    // An error handler that throws while describing an error takes the surrounding
    // tree down with it, so a throwing accessor is treated as an unknown error.
    try {
      return fromRecord(caught);
    } catch {
      return genericError();
    }
  }
  if (typeof caught === "string" && caught.trim() !== "") {
    return { code: "internal", message: caught, retryable: false };
  }
  return genericError();
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function fromRecord(caught: Record<string, unknown>): IpcError {
  const code = stringOrUndefined(caught["code"]);
  const message = stringOrUndefined(caught["message"]);
  const detail = stringOrUndefined(caught["detail"]);
  const cause = stringOrUndefined(caught["cause"]);
  return {
    code: code ?? "internal",
    message: message ?? GENERIC_MESSAGE,
    // Absent or non-boolean means the core did not offer a retry; never invent one.
    retryable: caught["retryable"] === true,
    // `exactOptionalPropertyTypes` distinguishes "absent" from "undefined", and a
    // consumer can then treat a missing `detail` as "the core sent none".
    ...(detail === undefined ? {} : { detail }),
    ...(cause === undefined ? {} : { cause }),
  };
}

/**
 * Last-resort message for an error the core did not describe. It is deliberately
 * not user-facing copy: the UI renders `ipc.error.internal.*` instead.
 */
export const GENERIC_MESSAGE = "the core returned an error with no message";

function genericError(): IpcError {
  return { code: "internal", message: GENERIC_MESSAGE, retryable: false };
}

function stringOrUndefined(value: unknown): string | undefined {
  return typeof value === "string" && value !== "" ? value : undefined;
}
