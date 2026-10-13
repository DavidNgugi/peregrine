import { createI18n } from "@peregrine/i18n";
import { describe, expect, it } from "vitest";
import {
  credentialFailureKey,
  credentialSuccessKey,
  type CredentialAction,
} from "./provider-outcome.js";

const i18n = createI18n();

/** The sentence a catalogue key renders to, with the name a sentence may carry. */
function sentence(key: string, provider = "TMDB"): string {
  return i18n.t(key, { provider });
}

describe("credentialFailureKey", () => {
  it("gives each code the job records the sentence a person acts on", () => {
    expect(sentence(credentialFailureKey("PROVIDER_UNAUTHORIZED"))).toBe(
      "That key was refused. Check you copied the API key and not another value.",
    );
    expect(sentence(credentialFailureKey("PROVIDER_RATE_LIMITED"))).toBe(
      "TMDB is rate limiting this app; try again in a minute.",
    );
    expect(sentence(credentialFailureKey("PROVIDER_UNREACHABLE"))).toBe(
      "TMDB could not be reached.",
    );
    expect(sentence(credentialFailureKey("PROVIDER_STORE"))).toBe(
      "Peregrine could not read its credential store, so the key was not saved.",
    );
    expect(sentence(credentialFailureKey("PROVIDER_NOT_CONFIGURED"))).toContain("nothing to test");
    expect(sentence(credentialFailureKey("CANCELLED"))).toContain("cancelled");
  });

  it("answers a code this build does not know like a failure with no code", () => {
    // A newer build can write a code this one does not read, and turning an unrecognised
    // string into a specific instruction would be guessing at what the provider said.
    const unknown = credentialFailureKey("PROVIDER_SOMETHING_NEW");

    expect(unknown).toBe(credentialFailureKey(null));
    expect(sentence(unknown)).toContain("could not be confirmed");
  });

  it("answers no code at all as the same generic failure", () => {
    expect(credentialFailureKey(null)).toBe(credentialFailureKey(undefined));
  });
});

describe("credentialSuccessKey", () => {
  it("names what each action did, so a success is not reported as a failure", () => {
    const actions: readonly CredentialAction[] = ["saved", "tested", "removed"];
    for (const action of actions) {
      const key = credentialSuccessKey(action);
      expect(sentence(key).length).toBeGreaterThan(0);
      expect(key).not.toContain("outcome.failed");
    }
    expect(sentence(credentialSuccessKey("removed"))).toContain("removed");
  });
});
