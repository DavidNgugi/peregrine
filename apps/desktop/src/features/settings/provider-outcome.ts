/**
 * What a finished credential job says, as pure functions.
 *
 * Separate from the hook for the reason `features/jobs/job-progress.ts` is separate from its
 * store: a Node test environment has no Tauri bridge, so whatever can be wrong has to be a
 * function of its inputs. What can be wrong here is the mapping from a stable code to the
 * sentence a person reads, and it is the whole of what the screen adds to the job's answer: the
 * code is the job's (`crates/peregrine-app/src/jobs/provider_credential.rs`), and a screen that
 * showed it raw would be showing a diagnostic label instead of an instruction.
 */

import type { CredentialStoreDto } from "@peregrine/ipc-client";

/**
 * The three things a credential job is started for.
 *
 * It decides which success sentence is shown, and nothing else: all three run the same job kind
 * and report the same failure codes.
 */
export type CredentialAction = "removed" | "saved" | "tested";

/**
 * The catalogue key of one provider's state, from its flag and what the keychain answered.
 *
 * **The flag is not the whole answer, and this is where that is decided.** A keychain that refused
 * access leaves every provider unconfigured exactly as a first run does, so a row that read the flag
 * alone would say "No key stored" to a user whose key is still there. The flag still wins when it is
 * set, because an adapter built from a key that was read once is still in hand and the session can
 * still ask that provider.
 */
export function providerStateKey(configured: boolean, store: CredentialStoreDto): string {
  if (configured) {
    return "settings.providers.configured";
  }
  switch (store) {
    case "answered":
      return "settings.providers.notConfigured";
    case "denied":
      return "settings.providers.store.denied";
    case "unavailable":
      return "settings.providers.store.unavailable";
    case "unread":
      return "settings.providers.store.unread";
  }
}

/**
 * The catalogue key of the section's notice about the keychain, or `null` when it answered.
 *
 * One sentence for the section rather than one per provider: whether the keychain could be read is a
 * fact about the system rather than about TMDB, and repeating it beside every provider would be the
 * same news once per row.
 */
export function storeNoticeKey(store: CredentialStoreDto): string | null {
  switch (store) {
    case "answered":
      return null;
    case "denied":
      return "settings.providers.storeNotice.denied";
    case "unavailable":
      return "settings.providers.storeNotice.unavailable";
    case "unread":
      return "settings.providers.storeNotice.unread";
  }
}

/** The catalogue key of what a job that succeeded did, by the action it was started for. */
const SUCCESS_KEYS: Readonly<Record<CredentialAction, string>> = {
  saved: "settings.providers.outcome.saved",
  tested: "settings.providers.outcome.working",
  removed: "settings.providers.outcome.removed",
};

/** The catalogue key of what a successful job did. */
export function credentialSuccessKey(action: CredentialAction): string {
  return SUCCESS_KEYS[action];
}

/**
 * The catalogue key of why a credential job failed.
 *
 * **A code this build does not know is answered like a failure with no code at all.** The journal
 * is the record and it may carry a code a newer build wrote, or none at all when the row could not
 * be read; the job reported that it did not succeed, and turning an unrecognised string into a
 * specific instruction would be guessing at what the provider said.
 */
export function credentialFailureKey(code: string | null | undefined): string {
  switch (code) {
    case "PROVIDER_UNAUTHORIZED":
      return "settings.providers.outcome.unauthorized";
    case "PROVIDER_RATE_LIMITED":
      return "settings.providers.outcome.rateLimited";
    case "PROVIDER_UNREACHABLE":
      return "settings.providers.outcome.unreachable";
    case "PROVIDER_STORE":
      return "settings.providers.outcome.store";
    case "PROVIDER_NOT_CONFIGURED":
      return "settings.providers.outcome.notConfigured";
    case "CANCELLED":
      return "settings.providers.outcome.cancelled";
    default:
      return "settings.providers.outcome.failed";
  }
}

/**
 * How many times a screen re-reads the credential status while the keychain has not answered.
 *
 * A read that has not finished is the ordinary state for the first moments of a launch — and on
 * macOS it can last as long as a permission dialog is on screen. The screen cannot tell those apart,
 * so it keeps asking for a bounded while and then stops and says so, rather than either giving up
 * immediately or polling a keychain for ever.
 */
export const POLL_ATTEMPTS = 15;

/** Whether another status read is worth making. */
export function shouldPoll(attempts: number): boolean {
  return attempts < POLL_ATTEMPTS;
}

/** Whether the keychain has still not answered, which is the state a screen waits in. */
export function stillReading(store: CredentialStoreDto): boolean {
  return store === "unread";
}
