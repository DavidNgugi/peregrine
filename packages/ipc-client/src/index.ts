// The generated half of the IPC contract. `commands` is the only value the
// bindings export today; an event is added to this module when the event bus
// gets its first topic (PG-E2-04), and the export gate regenerates this file.
export { commands } from "./generated/bindings.js";
export type { AppError, BuildInfo, ErrorCode } from "./generated/bindings.js";

export { getBuildInfo, isSuccessful } from "./client.js";
export {
  describeIpcError,
  GENERIC_MESSAGE,
  IPC_ERROR_CODES,
  IPC_ERROR_COPY,
  isIpcErrorCode,
  normalizeIpcError,
} from "./errors.js";
export type { IpcError, IpcErrorCopy, IpcErrorMessageKey } from "./errors.js";
