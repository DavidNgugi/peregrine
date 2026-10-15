import { createI18n } from "@peregrine/i18n";
import { describe, expect, it } from "vitest";
import {
  credentialFailureKey,
  credentialSuccessKey,
  POLL_ATTEMPTS,
  providerStateKey,
  shouldPoll,
  stillReading,
  storeNoticeKey,
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

describe("providerStateKey", () => {
  it("never claims no key is stored when the keychain could not be read", () => {
    // The reading a reinstall deleted something: a denied keychain leaves every flag false, so the
    // flag alone is not the answer.
    for (const store of ["denied", "unavailable", "unread"] as const) {
      expect(providerStateKey(false, store)).not.toBe("settings.providers.notConfigured");
    }
    expect(providerStateKey(false, "answered")).toBe("settings.providers.notConfigured");
  });

  it("lets the flag win, because an adapter read once is still in hand", () => {
    for (const store of ["answered", "denied", "unavailable", "unread"] as const) {
      expect(providerStateKey(true, store)).toBe("settings.providers.configured");
    }
  });

  it("has its own word for every state, so none is a silent fallback", () => {
    const words = (["answered", "denied", "unavailable", "unread"] as const).map((store) =>
      providerStateKey(false, store),
    );
    expect(new Set(words).size).toBe(4);
  });
});

describe("storeNoticeKey", () => {
  it("says nothing when the keychain answered", () => {
    expect(storeNoticeKey("answered")).toBeNull();
  });

  it("has a sentence for each way it did not", () => {
    expect(storeNoticeKey("denied")).toBe("settings.providers.storeNotice.denied");
    expect(storeNoticeKey("unavailable")).toBe("settings.providers.storeNotice.unavailable");
    expect(storeNoticeKey("unread")).toBe("settings.providers.storeNotice.unread");
  });
});

describe("the wait for a keychain that has not answered", () => {
  it("keeps asking for a bounded while and then stops", () => {
    // A read that has not finished can be a launch that is a moment old or a permission dialog that
    // is on screen; the screen cannot tell them apart, so it polls for a while and then says so
    // instead of either giving up at once or polling a keychain for ever.
    expect(shouldPoll(0)).toBe(true);
    expect(shouldPoll(POLL_ATTEMPTS - 1)).toBe(true);
    expect(shouldPoll(POLL_ATTEMPTS)).toBe(false);
    expect(shouldPoll(POLL_ATTEMPTS + 100)).toBe(false);
  });

  it("waits only on the state that has not answered", () => {
    expect(stillReading("unread")).toBe(true);
    for (const store of ["answered", "denied", "unavailable"] as const) {
      expect(stillReading(store)).toBe(false);
    }
  });
});
