import { createI18n } from "@peregrine/i18n";
import { renderToStaticMarkup } from "react-dom/server";
import { I18nextProvider } from "react-i18next";
import { describe, expect, it } from "vitest";
import {
  integrationFailureKey,
  priorityMove,
  type IntegrationDto,
  type IntegrationSettings,
} from "../hooks/useIntegrations.js";
import { IntegrationsSection } from "./IntegrationsSection.js";

const i18n = createI18n();

/** One integration as the core sends it, with only the fields a case cares about overridden. */
function integration(overrides: Partial<IntegrationDto> = {}): IntegrationDto {
  return {
    id: "tmdb",
    displayName: "TMDB",
    description: "Title, season, episode, and artwork metadata. A key is required.",
    docs: "https://developer.themoviedb.org/docs",
    category: "metadata",
    capabilities: ["metadata"],
    requiresKey: true,
    keyOptional: false,
    submissionSupport: false,
    alwaysOn: false,
    enabled: false,
    priority: 0,
    submissionEnabled: false,
    configured: false,
    needsAttention: false,
    store: "answered",
    lastOutcome: null,
    lastTestAt: null,
    lastErrorCode: null,
    fields: [],
    ...overrides,
  };
}

/** The read as a state, with every action recorded rather than performed. */
function settings(state: Partial<IntegrationSettings> = {}): IntegrationSettings {
  return {
    integrations: [],
    loading: false,
    failed: false,
    busy: false,
    outcome: null,
    pending: null,
    error: null,
    reload: () => undefined,
    setEnabled: () => Promise.resolve(),
    setSubmission: () => Promise.resolve(),
    move: () => Promise.resolve(),
    testConnection: () => Promise.resolve(),
    saveCredentials: () => Promise.resolve(),
    removeCredentials: () => Promise.resolve(),
    ...state,
  };
}

/**
 * The section as markup rather than as a mounted tree.
 *
 * The cards are a projection of the read and nothing else, so static rendering is what this
 * component has to get right; the hook's job is exercised through the IPC bridge rather than
 * here.
 */
function markup(state: IntegrationSettings): string {
  return renderToStaticMarkup(
    <I18nextProvider i18n={i18n}>
      <IntegrationsSection integrations={state} />
    </I18nextProvider>,
  );
}

/**
 * Whether the button whose accessible name is `label` is disabled.
 *
 * The label is an attribute rather than text, and `disabled=""` is the attribute rather than the
 * word: Tailwind's `disabled:` variants put the word in every class list.
 */
function disabledControl(html: string, label: string): boolean {
  const at = html.indexOf(`aria-label="${label}"`);
  if (at === -1) {
    return false;
  }
  return html.slice(html.lastIndexOf("<button", at), at).includes('disabled=""');
}

/** Whether the button whose visible text is `label` is disabled. */
function disabledButton(html: string, label: string): boolean {
  const end = html.lastIndexOf(`>${label}</button>`);
  if (end === -1) {
    return false;
  }
  return html.slice(html.lastIndexOf("<button", end), end).includes('disabled=""');
}

describe("IntegrationsSection", () => {
  it("says a public source needs no key and marks its credentials optional", () => {
    const intro = integration({
      id: "theintrodb",
      displayName: "TheIntroDB",
      category: "skipMarkers",
      capabilities: ["markers"],
      requiresKey: false,
      keyOptional: true,
      submissionSupport: true,
      configured: true,
      priority: 10,
      fields: [
        {
          name: "apiKey",
          label: "User API key (optional, for submissions)",
          secret: true,
          required: false,
          present: false,
        },
      ],
    });
    const html = markup(settings({ integrations: [intro] }));

    expect(html).toContain("Key optional");
    expect(html).not.toContain("No key required");
    // Configured is the core's flag, not the presence of a key: public reads need none.
    expect(html).toContain(">Configured</span>");
    expect(html).not.toContain(">Not configured</span>");
    // Its contribution toggle exists, and it starts off until the user consents.
    expect(html).toContain("Allow submissions from TheIntroDB");
    expect(html).toContain('aria-checked="false"');
    expect(html).toContain('role="switch"');
  });

  it("draws no credential field for a source that has none", () => {
    const html = markup(
      settings({
        integrations: [
          integration({
            id: "aniskip",
            displayName: "AniSkip",
            category: "skipMarkers",
            capabilities: ["markers"],
            requiresKey: false,
            configured: true,
            priority: 20,
          }),
        ],
      }),
    );

    expect(html).toContain("No key required");
    expect(html).not.toContain('type="password"');
  });

  it("asks for TMDB's key and refuses a save while the field is empty", () => {
    const html = markup(
      settings({
        integrations: [
          integration({
            fields: [
              {
                name: "apiKey",
                label: "API key",
                secret: true,
                required: true,
                present: false,
              },
            ],
          }),
        ],
      }),
    );

    expect(html).toContain('type="password"');
    expect(html).toContain("API key");
    expect(html).toContain(">Save</button>");
    expect(html).not.toContain(">Replace</button>");
    expect(disabledButton(html, "Save")).toBe(true);
  });

  it("says the keychain refused the store rather than that nothing is configured", () => {
    // A denied keychain leaves TMDB unconfigured exactly as a first run does; saying "Not
    // configured" here is the sentence that makes a reinstall look like a deletion.
    const html = markup(
      settings({
        integrations: [
          integration({
            store: "denied",
            fields: [
              {
                name: "apiKey",
                label: "API key",
                secret: true,
                required: true,
                present: false,
              },
            ],
          }),
        ],
      }),
    );

    expect(html).toContain("Keychain access refused");
    expect(html).not.toContain(">Not configured</span>");
    expect(html).not.toContain("No key stored");
  });

  it("shows local chapters as always on and offers no switch", () => {
    const html = markup(
      settings({
        integrations: [
          integration({
            id: "chapters",
            displayName: "Local chapters",
            category: "skipMarkers",
            capabilities: ["markers"],
            requiresKey: false,
            alwaysOn: true,
            enabled: true,
            configured: true,
            priority: 0,
          }),
        ],
      }),
    );

    expect(html).toContain("Always on");
    expect(html).not.toContain('role="switch"');
  });

  it("moves the middle priority by its neighbour's value and disables the ends", () => {
    const rows: readonly IntegrationDto[] = [
      integration({
        id: "chapters",
        displayName: "Local chapters",
        category: "skipMarkers",
        capabilities: ["markers"],
        requiresKey: false,
        alwaysOn: true,
        configured: true,
        priority: 0,
      }),
      integration({
        id: "theintrodb",
        displayName: "TheIntroDB",
        category: "skipMarkers",
        capabilities: ["markers"],
        requiresKey: false,
        keyOptional: true,
        configured: true,
        priority: 10,
        fields: [
          {
            name: "apiKey",
            label: "User API key (optional, for submissions)",
            secret: true,
            required: false,
            present: false,
          },
        ],
      }),
      integration({
        id: "aniskip",
        displayName: "AniSkip",
        category: "skipMarkers",
        capabilities: ["markers"],
        requiresKey: false,
        configured: true,
        priority: 20,
      }),
    ];
    const html = markup(settings({ integrations: rows }));

    // Even with an always-on row in the category, the neighbour is the one above it.
    expect(disabledControl(html, "Move Local chapters up")).toBe(true);
    expect(disabledControl(html, "Move Local chapters down")).toBe(false);
    expect(disabledControl(html, "Move TheIntroDB up")).toBe(false);
    expect(disabledControl(html, "Move TheIntroDB down")).toBe(false);
    expect(disabledControl(html, "Move AniSkip down")).toBe(true);

    // A move is a swap: the moved row takes the neighbour's place and the neighbour takes the
    // moved row's value, so the rendered order changes rather than ties.
    expect(priorityMove(rows, "theintrodb", "up")).toEqual({
      id: "theintrodb",
      priority: 0,
      swapId: "chapters",
      swapPriority: 10,
    });
    expect(priorityMove(rows, "theintrodb", "down")).toEqual({
      id: "theintrodb",
      priority: 20,
      swapId: "aniskip",
      swapPriority: 10,
    });
    expect(priorityMove(rows, "chapters", "up")).toBeNull();
    expect(priorityMove(rows, "aniskip", "down")).toBeNull();
  });

  it("shows a toast while a connection test runs and after it finishes", () => {
    const row = integration({
      id: "theintrodb",
      displayName: "TheIntroDB",
      requiresKey: false,
      keyOptional: true,
      configured: true,
      category: "skipMarkers",
      capabilities: ["markers"],
    });

    const running = markup(
      settings({
        integrations: [row],
        pending: { integration: "theintrodb", action: "tested" },
      }),
    );
    expect(running).toContain('role="status"');
    expect(running).toContain("Testing TheIntroDB…");

    const succeeded = markup(
      settings({
        integrations: [row],
        outcome: {
          key: "settings.integrations.outcome.tested",
          integration: "theintrodb",
          ok: true,
        },
      }),
    );
    expect(succeeded).toContain('role="status"');
    expect(succeeded).toContain("The connection to TheIntroDB works.");

    const failed = markup(
      settings({
        integrations: [row],
        outcome: {
          key: "settings.integrations.outcome.unreachable",
          integration: "theintrodb",
          ok: false,
        },
      }),
    );
    expect(failed).toContain('role="alert"');
    expect(failed).toContain("TheIntroDB could not be reached.");
  });

  it("turns a terminal job failure code into one sentence key", () => {
    expect(integrationFailureKey("INTEGRATION_UNAUTHORIZED")).toBe(
      "settings.integrations.outcome.unauthorized",
    );
    expect(integrationFailureKey("INTEGRATION_RATE_LIMITED")).toBe(
      "settings.integrations.outcome.rateLimited",
    );
    expect(integrationFailureKey("INTEGRATION_STORE")).toBe("settings.integrations.outcome.store");
    expect(integrationFailureKey("CANCELLED")).toBe("settings.integrations.outcome.cancelled");
    // A code a newer build wrote is answered generically rather than guessed at.
    expect(integrationFailureKey("SOMETHING_NEW")).toBe("settings.integrations.outcome.failed");
    expect(integrationFailureKey(null)).toBe("settings.integrations.outcome.failed");
  });
});
