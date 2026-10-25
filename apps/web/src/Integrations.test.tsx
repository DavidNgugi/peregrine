import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import type { IntegrationStatus } from "./api.js";
import {
  credentialFact,
  describeStatus,
  enabledLabel,
  groupIntegrations,
  IntegrationsView,
} from "./Integrations.js";

function integration(overrides: Partial<IntegrationStatus> = {}): IntegrationStatus {
  return {
    id: "chapters",
    displayName: "Local chapters",
    description: "The file's own chapter names, read with ffprobe.",
    category: "skipMarkers",
    enabled: true,
    configured: true,
    requiresKey: false,
    keyOptional: false,
    alwaysOn: true,
    lastOutcome: null,
    needsAttention: false,
    ...overrides,
  };
}

const ROWS: readonly IntegrationStatus[] = [
  integration({
    id: "tmdb",
    displayName: "TMDB",
    description: "Title, season, episode, and artwork metadata. A key is required.",
    category: "metadata",
    enabled: false,
    configured: false,
    alwaysOn: false,
    needsAttention: true,
  }),
  integration(),
  integration({
    id: "theintrodb",
    displayName: "TheIntroDB",
    description: "Community intro, recap, credits, and preview timestamps.",
    alwaysOn: false,
    lastOutcome: "working",
  }),
  integration({
    id: "aniskip",
    displayName: "AniSkip",
    description: "Anime intro and ending timestamps.",
    alwaysOn: false,
    lastOutcome: "rateLimited",
    needsAttention: true,
  }),
  integration({
    id: "trakt",
    displayName: "Trakt",
    description: "Watch-history scrobbling.",
    category: "tracking",
    enabled: false,
    configured: true,
    alwaysOn: false,
    lastOutcome: "unauthorized",
    needsAttention: true,
  }),
];

function render(
  overrides: { rows?: readonly IntegrationStatus[]; loading?: boolean; error?: string | null } = {},
): string {
  return renderToStaticMarkup(
    <IntegrationsView
      error={overrides.error ?? null}
      integrations={overrides.rows ?? ROWS}
      loading={overrides.loading ?? false}
      onRetry={() => {}}
    />,
  );
}

describe("groupIntegrations", () => {
  it("groups in the desktop's category order and omits empty groups", () => {
    const groups = groupIntegrations(ROWS);
    expect(groups.map((group) => group.category)).toEqual(["metadata", "skipMarkers", "tracking"]);
    expect(groups[1]?.items.map((row) => row.id)).toEqual(["chapters", "theintrodb", "aniskip"]);
  });

  it("returns nothing for an empty answer", () => {
    expect(groupIntegrations([])).toEqual([]);
  });
});

describe("status presentation", () => {
  it("names every outcome a probe can return", () => {
    expect(describeStatus(integration({ lastOutcome: "working" }))).toEqual({
      label: "Working",
      tone: "ok",
      detail: null,
    });
    expect(describeStatus(integration({ lastOutcome: "rateLimited" })).label).toBe("Rate limited");
    for (const outcome of ["unauthorized", "unreachable", "keychainDenied"]) {
      expect(describeStatus(integration({ lastOutcome: outcome })).label).toBe("Needs attention");
    }
    expect(describeStatus(integration({ lastOutcome: "notConfigured" })).label).toBe(
      "Not configured",
    );
  });

  it("falls back to the configuration state when no probe has run", () => {
    expect(describeStatus(integration()).label).toBe("Configured");
    expect(describeStatus(integration({ configured: false })).label).toBe("Not configured");
    expect(describeStatus(integration({ configured: false, needsAttention: true })).label).toBe(
      "Not configured",
    );
  });

  it("treats an unknown outcome as attention rather than success", () => {
    expect(describeStatus(integration({ lastOutcome: "newFailureMode" })).label).toBe(
      "Needs attention",
    );
  });
});

describe("enabled and credential copy", () => {
  it("says always on for chapters and enabled or disabled for the rest", () => {
    expect(enabledLabel(integration())).toBe("Always on");
    expect(enabledLabel(integration({ alwaysOn: false, enabled: true }))).toBe("Enabled");
    expect(enabledLabel(integration({ alwaysOn: false, enabled: false }))).toBe("Disabled");
  });

  it("says no key required only where the answer proves it", () => {
    expect(credentialFact(integration())).toBe("No key required");
    expect(credentialFact(integration({ requiresKey: true }))).toBeNull();
    expect(credentialFact(integration({ requiresKey: false, keyOptional: true }))).toBe(
      "No key required; a key is optional",
    );
  });
});

describe("IntegrationsView", () => {
  it("draws the grouped cards with status, state, and the desktop instruction", () => {
    const html = render();
    expect(html).toContain("Metadata");
    expect(html).toContain("Skip markers");
    expect(html).toContain("Tracking");
    expect(html.indexOf("Metadata")).toBeLessThan(html.indexOf("Skip markers"));
    expect(html).toContain("TMDB");
    expect(html).toContain("Local chapters");
    expect(html).toContain("TheIntroDB");
    expect(html).toContain("AniSkip");
    expect(html).toContain("Trakt");
    expect(html).toContain("Working");
    expect(html).toContain("Rate limited");
    expect(html).toContain("Not configured");
    expect(html).toContain("Needs attention");
    expect(html).toContain("Always on");
    expect(html).toContain("Enabled");
    expect(html).toContain("Disabled");
    expect(html).toContain("No key required");
    expect(html.match(/Manage credentials on desktop\./gu)).toHaveLength(ROWS.length);
    expect(html).toContain('data-focus="true"');
    expect(html).toContain('data-autofocus="true"');
  });

  it("renders no credential field of any kind", () => {
    const html = render().toLocaleLowerCase();
    expect(html).not.toContain("<input");
    expect(html).not.toContain("password");
    expect(html).not.toContain("<textarea");
    expect(html).not.toContain("<select");
  });

  it("draws an empty state rather than an empty page", () => {
    const html = render({ rows: [] });
    expect(html).toContain("No integrations are available in this build.");
  });

  it("draws the read failure and a retry control", () => {
    const html = render({ rows: [], error: "The listener refused the read." });
    expect(html).toContain("The listener refused the read.");
    expect(html).toContain("Try again");
  });

  it("draws a reading state before the first answer", () => {
    expect(render({ rows: [], loading: true })).toContain("Reading integrations...");
  });
});
