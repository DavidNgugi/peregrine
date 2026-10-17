/**
 * Whether this installation has ever been set up.
 *
 * The rule is deliberately narrow: **no library root and no configured provider**. A folder alone
 * is enough to make the application useful, and a provider alone is enough to browse the
 * catalogue, so either one means the user has already answered the question the wizard asks.
 *
 * The keychain's own state is not consulted. A keychain that refused access leaves every
 * `configured` flag false exactly as a first run does — but a returner with a library still has a
 * root, and a first run with no roots has nothing to show whether a key is in there or not. Reading
 * `CredentialStoreDto` here would turn "nobody could look" into a reason to skip setup, which is
 * the one direction this check must not fail in.
 */

import type { LibraryRootDto, ProviderStatusDto } from "@peregrine/ipc-client";

/**
 * What the first-run read answered, or why it could not answer.
 *
 * `unknown` is distinct from `checking` and from `configured` on purpose: a failed read is not
 * evidence of a first run, so the gate treats it as a returner and leaves the application usable.
 * Asking someone with an existing library to set up again because an IPC call failed would be a
 * worse failure than showing the empty library they can repair from Settings.
 */
export type FirstRunCheck =
  | { readonly state: "checking" }
  | { readonly state: "first-run" }
  | { readonly state: "configured" }
  | { readonly state: "unknown" };

/** Whether the read found no library root and no configured provider. */
export function isFirstRun(
  roots: readonly LibraryRootDto[],
  providers: ProviderStatusDto,
): boolean {
  return roots.length === 0 && !providers.known.some((entry) => entry.configured);
}
