/**
 * What a failed call says to a user, in one place.
 *
 * The core writes `detail` as a sentence meant to be read — "That folder is already one of your
 * libraries." — while `message` is the code's own short word for the same failure. A screen that
 * showed `message` would be showing the user a diagnostic label; a screen that showed `detail` when
 * there is none would show nothing at all. `describeIpcError` is the last resort: it answers with
 * catalogue keys rather than text, and the fallback is its title key, which the i18n catalogue
 * always defines.
 *
 * Two features needed this rule and a rule with two implementations is a rule that gets fixed once.
 */

import { describeIpcError } from "@peregrine/ipc-client";
import type { IpcError } from "@peregrine/ipc-client";

/**
 * The sentence to show for a failed call.
 *
 * An empty string counts as absent rather than as an answer: a core that sent `message: ""` has said
 * nothing, and a message box with nothing in it is worse than one naming the failure's family.
 */
export function messageOf(error: IpcError): string {
  const written = [error.detail, error.message].find(
    (part) => part !== undefined && part.trim() !== "",
  );
  return written ?? describeIpcError(error).titleKey;
}
