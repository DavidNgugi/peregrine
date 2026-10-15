import { createI18n } from "@peregrine/i18n";
import type { ProviderDto } from "@peregrine/ipc-client";
import { renderToStaticMarkup } from "react-dom/server";
import { I18nextProvider } from "react-i18next";
import { describe, expect, it } from "vitest";
import type { ProviderSettings } from "../hooks/useProviders.js";
import { credentialFailureKey, credentialSuccessKey } from "../provider-outcome.js";
import { ProvidersSection } from "./ProvidersSection.js";

const i18n = createI18n();

/** One provider as the core sends it. */
function provider(overrides: Partial<ProviderDto> = {}): ProviderDto {
  return {
    provider: "tmdb",
    displayName: "TMDB",
    endpoint: "https://api.themoviedb.org/3",
    configured: false,
    ...overrides,
  };
}

/** The read as a state, with the three operations recorded rather than performed. */
function settings(state: Partial<ProviderSettings> = {}): ProviderSettings {
  return {
    providers: [],
    // The ordinary case: the keychain answered, so a provider's flag is the whole truth.
    store: "answered",
    loading: false,
    failed: false,
    busy: false,
    outcome: null,
    error: null,
    saveKey: () => Promise.resolve(),
    testConnection: () => Promise.resolve(),
    removeKey: () => Promise.resolve(),
    reload: () => undefined,
    ...state,
  };
}

/**
 * The section as markup rather than as a mounted tree.
 *
 * The list is a projection of the read and nothing else, so static rendering is what this
 * component has to get right; what a click does is the hook's, and the hook is exercised
 * through the IPC bridge rather than here.
 */
function markup(state: ProviderSettings): string {
  return renderToStaticMarkup(
    <I18nextProvider i18n={i18n}>
      <ProvidersSection providers={state} />
    </I18nextProvider>,
  );
}

/**
 * Whether the button drawing `label` is disabled, read from the markup.
 *
 * The label appears more than once per section — as a row's label, as a button's text, and in
 * the confirmation — so counting occurrences says nothing about the control. This reads the one
 * button instead, taking the last match because the confirmation is drawn after the rows.
 */
function disabledButton(html: string, label: string): boolean {
  const end = html.lastIndexOf(`>${label}</button>`);
  if (end === -1) {
    return false;
  }
  // The attribute, not the word: the button's class list carries Tailwind's `disabled:` variants,
  // so a search for `disabled` matches every button ever drawn.
  return html.slice(html.lastIndexOf("<button", end), end).includes(`disabled=""`);
}

describe("ProvidersSection", () => {
  it("names each known provider with its endpoint and whether it is configured", () => {
    const html = markup(
      settings({ providers: [provider(), provider({ provider: "other", displayName: "Other" })] }),
    );

    expect(html).toContain("TMDB");
    expect(html).toContain("https://api.themoviedb.org/3");
    expect(html).toContain("No key stored");
    expect(html).toContain("Other");
  });

  it("says a provider is configured when the core says so", () => {
    const unconfigured = markup(settings({ providers: [provider()] }));
    const configured = markup(settings({ providers: [provider({ configured: true })] }));

    expect(unconfigured).toContain("No key stored");
    expect(unconfigured).not.toContain("Configured");
    expect(configured).toContain("Configured");
    expect(configured).not.toContain("No key stored");
  });

  it("offers the connection test only for a provider that holds a key", () => {
    // A test of nothing is a request the provider would refuse, and the core would answer
    // PROVIDER_NOT_CONFIGURED after spending a job on it.
    const unconfigured = markup(settings({ providers: [provider()] }));
    const configured = markup(settings({ providers: [provider({ configured: true })] }));

    expect(unconfigured).toContain("Save and test");
    expect(unconfigured).not.toContain("Test connection");
    expect(configured).toContain("Test connection");
    expect(configured).toContain("Remove");
  });

  it("says the keychain refused access rather than that no key is stored", () => {
    // **The sentence this test exists for.** macOS binds a keychain item's access to the code
    // signature of the application that created it, so a rebuilt or reinstalled binary is a
    // different application and is asked; a denial leaves every provider unconfigured exactly as a
    // first run does. A row that said "No key stored" there is what makes a user believe a reinstall
    // deleted something that is still in their keychain.
    const denied = markup(settings({ providers: [provider()], store: "denied" }));

    expect(denied).toContain("Keychain access refused");
    expect(denied).toContain("Nothing has been deleted");
    expect(denied).toContain("Keychain Access");
    expect(denied).not.toContain("No key stored");
    expect(denied).toContain("Try again");
  });

  it("says there is no keychain on this system rather than that the key is gone", () => {
    const unavailable = markup(settings({ providers: [provider()], store: "unavailable" }));

    expect(unavailable).toContain("No keychain available");
    expect(unavailable).toContain("not available in this session");
    expect(unavailable).not.toContain("No key stored");
  });

  it("claims nothing about what is stored before the keychain has been read", () => {
    // The state a launch is in until the startup read answers. A settings screen opened inside that
    // window is the other half of the same defect: it was saying "No key stored" about a store
    // nobody had looked in yet.
    const unread = markup(settings({ providers: [provider()], store: "unread" }));

    expect(unread).toContain("has not finished reading your keychain");
    expect(unread).toContain("Checking the keychain");
    expect(unread).toContain("does not delete the key");
    expect(unread).not.toContain("No key stored");
    // And a way to ask again without reopening the app, because the request may just have been
    // allowed.
    expect(unread).toContain("Try again");
  });

  it("still says a provider is configured when a later read was refused", () => {
    // The adapters built from a key that was read once are still in hand, so the session can still
    // ask that provider. The section explains the store; it does not overrule the flag — which is why
    // the row keeps "Configured" and the keychain sentence is the section's rather than the row's.
    const refused = markup(
      settings({ providers: [provider({ configured: true })], store: "denied" }),
    );

    expect(refused).toContain("Configured");
    expect(refused).not.toContain("No key stored");
    expect(refused).toContain("Nothing has been deleted");
  });

  it("offers no retry once the keychain has answered", () => {
    // Nothing to ask again: the store answered, and a control that re-read it would be a button that
    // does nothing visible.
    const answered = markup(settings({ providers: [provider()] }));

    expect(answered).not.toContain("Try again");
  });

  it("says where the key lives, so a reinstall is not mistaken for a deletion", () => {
    const html = markup(settings({ providers: [provider()] }));

    expect(html).toContain("keychain");
    expect(html).toContain("reinstalling Peregrine does not remove it");
  });

  it("writes the key field as a password field, never in clear text", () => {
    const html = markup(settings({ providers: [provider()] }));

    expect(html).toContain('type="password"');
    expect(html).not.toContain('type="text"');
  });

  it("distinguishes a read that failed from a list that is genuinely empty", () => {
    const empty = markup(settings());
    const failed = markup(settings({ failed: true }));

    expect(empty).toContain("No metadata provider is built in.");
    expect(empty).not.toContain("could not be read");
    expect(failed).toContain("could not be read");
    expect(failed).not.toContain("No metadata provider is built in.");
  });

  it("shows its own state while a job runs rather than a full-page spinner", () => {
    const html = markup(settings({ busy: true, providers: [provider({ configured: true })] }));

    expect(html).toContain("Asking the provider whether that key works…");
    expect(disabledButton(html, "Save and test")).toBe(true);
    expect(disabledButton(html, "Test connection")).toBe(true);
    expect(disabledButton(html, "Remove")).toBe(true);
  });

  it("reports a refused key with the sentence for the code the job recorded", () => {
    const html = markup(
      settings({
        providers: [provider()],
        outcome: {
          key: credentialFailureKey("PROVIDER_UNAUTHORIZED"),
          provider: "tmdb",
          ok: false,
        },
      }),
    );

    expect(html).toContain(
      "That key was refused. Check you copied the API key and not another value.",
    );
    expect(html).toContain('role="alert"');
    expect(html).not.toContain("PROVIDER_UNAUTHORIZED");
  });

  it("reports a stored key as news rather than as a failure", () => {
    const html = markup(
      settings({
        providers: [provider({ configured: true })],
        outcome: { key: credentialSuccessKey("saved"), provider: "tmdb", ok: true },
      }),
    );

    expect(html).toContain("That key works. Peregrine stored it for TMDB.");
    expect(html).toContain('role="status"');
    expect(html).not.toContain('role="alert"');
  });

  it("shows the refusal the core made before a job could start", () => {
    const html = markup(
      settings({ providers: [provider()], error: "Paste your API key before saving." }),
    );

    expect(html).toContain("Paste your API key before saving.");
    expect(html).toContain('role="alert"');
  });
});
