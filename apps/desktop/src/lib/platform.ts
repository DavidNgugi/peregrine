import { normalizeIpcError } from "@peregrine/ipc-client";
import { isTauri } from "@tauri-apps/api/core";
import { getBuildInfo } from "@peregrine/ipc-client";
import { logUiError } from "./error-log.js";

/**
 * Which platform's window chrome the shell presents.
 *
 * The three names are exactly what `std::env::consts::OS` returns, which is what the
 * core builds `BuildInfo.target` from (`crates/peregrine-ipc/src/system.rs`), so the two
 * sides cannot spell a platform differently without one of them failing to compile or
 * one of the tests below failing.
 */
export type Platform = "macos" | "windows" | "linux";

/**
 * Narrows a `BuildInfo.target` value (`"<os>-<arch>"`, from the generated bindings) to a
 * platform.
 *
 * Only the OS segment matters to the chrome and the match is exact: a target this shell
 * has no chrome for becomes `null` rather than the closest guess, and so does a value the
 * core never sends. Falling back to a platform would present macOS inset geometry on
 * Windows, which is worse than a neutral bar.
 */
export function parsePlatform(target: string): Platform | null {
  const [os] = target.split("-", 1);
  switch (os) {
    case "macos":
    case "windows":
    case "linux":
      return os;
    default:
      return null;
  }
}

/**
 * The running platform, read through the generated IPC contract rather than sniffed from
 * the user agent.
 *
 * `null` covers the two ways there is no answer: outside the Tauri shell (a browser
 * preview of the frontend) and a call that failed. Both leave the shell running with the
 * platform-neutral chrome, because a failed read must never stop the shell from starting
 * (docs/ARCHITECTURE.md §15). The reason to read `BuildInfo` rather than a window or OS
 * plugin is that the core is the only component that knows what it was compiled for, and
 * `build_info` is a lookup with no failing path, so the answer arrives before the first
 * paint.
 */
export async function resolvePlatform(): Promise<Platform | null> {
  if (!isTauri()) {
    return null;
  }
  try {
    return parsePlatform((await getBuildInfo()).target);
  } catch (caught) {
    logUiError("resolvePlatform", normalizeIpcError(caught));
    return null;
  }
}
