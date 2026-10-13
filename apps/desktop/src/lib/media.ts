/**
 * The typed frontend API for cached images and local thumbnails (PG-E2-08).
 *
 * This module is the only place in the app that names `convertFileSrc`. A feature that
 * needs a poster, a backdrop, a still, or a thumbnail calls `imageUrl` here, so the
 * protocol is named once, the return type stays honest, and there is one place to audit
 * for what a DTO field is allowed to become.
 *
 * **AGENTS R11 is why this module exists at all.** Image bytes never cross IPC: the core
 * writes them into its image cache and IPC carries a reference, which this module turns
 * into a URL the webview fetches directly over the `asset:` protocol. That is also why
 * `imageUrl` refuses anything that is not an absolute filesystem path — a `data:` URI
 * would be bytes on the wire, and a relative path would be resolved against the webview's
 * own document rather than against the cache.
 *
 * **This module is not the security boundary.** The Rust side is:
 * `apps/desktop/src-tauri/src/media.rs` allows exactly one directory on the asset
 * protocol scope, and Tauri's asset handler refuses every path outside it. The shape check
 * here exists so a mistake fails early and visibly — a relative path, a URL, or a
 * traversal renders nothing rather than asking the protocol a question it will refuse
 * anyway with an error nobody reads.
 */

import { convertFileSrc, isTauri } from "@tauri-apps/api/core";

/**
 * An absolute path to a file the core wrote inside its image cache.
 *
 * The brand is what stops an arbitrary DTO string from being passed where a cache
 * reference is wanted: the core produces the path, `isImagePath` narrows it at the
 * boundary, and no feature assembles one by hand.
 */
export type ImagePath = string & { readonly __imagePath: unique symbol };

/**
 * The Windows absolute forms that have no leading slash: a drive path, and a UNC share.
 *
 * Both are reachable because the cache directory is the OS one: a profile whose `LOCALAPPDATA` is
 * redirected to a file server resolves to a `\\server\share` path, and canonicalizing that gives the
 * verbatim form. A poster missing on such a machine, with nothing to report it, is the failure this
 * pair of patterns exists to prevent.
 */
const WINDOWS_ABSOLUTE = /^(?:[A-Za-z]:[\\/]|\\\\[^\\/])/;

/** The verbatim prefix Windows canonicalization puts in front of a local path, and of a UNC one. */
const VERBATIM_PREFIX = "\\\\?\\";
const VERBATIM_UNC_PREFIX = "\\\\?\\UNC\\";

/**
 * A Windows path with the verbatim prefix removed, for the shape check only.
 *
 * **`std::fs::canonicalize` answers `\\?\C:\…` on Windows**, and that is the path the shell hands
 * the runtime and the runtime answers a screen with, so a check that knew only the plain form would
 * refuse every cached poster on the one platform whose resolved paths carry a prefix — silently,
 * because a refused reference renders the same placeholder a missing poster does. Stripping it here is
 * what Tauri's own `convertFileSrc` does to the same path before it builds a URL, so the check and the
 * protocol agree about what the value is. The value itself is passed through untouched: the scope
 * canonicalizes what it is asked about, and both forms resolve to the same file.
 */
function withoutVerbatimPrefix(value: string): string {
  if (value.startsWith(VERBATIM_UNC_PREFIX)) {
    return `\\\\${value.slice(VERBATIM_UNC_PREFIX.length)}`;
  }
  return value.startsWith(VERBATIM_PREFIX) ? value.slice(VERBATIM_PREFIX.length) : value;
}

/**
 * Whether `value` is a plausible cache reference rather than a URL or a relative path.
 *
 * Deliberately structural: it can decide that a value is *not* a path, which is what keeps
 * a base64 payload or an `https:` URL out of an `<img>`; it cannot decide that a path is
 * inside the cache, because only the Rust scope can answer that.
 */
export function isImagePath(value: unknown): value is ImagePath {
  if (typeof value !== "string" || value.length === 0) {
    return false;
  }
  // A NUL truncates the path inside the OS, so a value carrying one is not the path it
  // appears to be; a control character is the same class of mistake.
  // eslint-disable-next-line no-control-regex
  if (/[\u0000-\u001f]/.test(value)) {
    return false;
  }
  const path = withoutVerbatimPrefix(value);
  if (!path.startsWith("/") && !WINDOWS_ABSOLUTE.test(path)) {
    return false;
  }
  return !path.split(/[\\/]/).includes("..");
}

/**
 * Turns a cache reference from the core into a URL the webview loads directly.
 *
 * `null` means "render nothing": the caller is outside the Tauri shell (a browser preview
 * of the frontend has no asset protocol), or the value is not a cache reference. There is
 * no fallback to a data URL — inventing bytes here is exactly what R11 forbids.
 */
export function imageUrl(reference: unknown): string | null {
  if (!isImagePath(reference) || !isTauri()) {
    return null;
  }
  return convertFileSrc(reference);
}
