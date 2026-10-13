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

/**
 * The three things a credential job is started for.
 *
 * It decides which success sentence is shown, and nothing else: all three run the same job kind
 * and report the same failure codes.
 */
export type CredentialAction = "removed" | "saved" | "tested";

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
