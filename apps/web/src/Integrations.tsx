/**
 * The read-only integrations screen (ADR 0028/0033).
 *
 * A television or phone can say which sources this installation has, whether they are enabled, and
 * what the last probe answered. It cannot edit a credential, test a source, or reorder one: the
 * desktop owns every write, and this screen's only instruction is where to go to make a change.
 * The web answer deliberately carries no field presence and no secret, so nothing here has an
 * input to leak.
 */

import { useEffect, useState, type ReactElement } from "react";

import { api, type IntegrationStatus } from "./api.js";
import { useAutoFocus } from "./focus.js";

type Category = IntegrationStatus["category"];

/** The order a person sees the groups in; it follows the desktop settings screen. */
const CATEGORY_ORDER: readonly Category[] = [
  "metadata",
  "skipMarkers",
  "subtitles",
  "tracking",
  "acquisition",
];

/** One non-empty group of cards. */
export interface IntegrationGroup {
  readonly category: Category;
  readonly label: string;
  readonly items: readonly IntegrationStatus[];
}

/** A status chip's colour; the label always carries the meaning on its own. */
export type StatusTone = "ok" | "warn" | "danger" | "neutral";

/** What the status chip says and why. */
export interface StatusDescriptor {
  readonly label: string;
  readonly tone: StatusTone;
  readonly detail: string | null;
}

/** The heading for one category. */
function categoryLabel(category: Category): string {
  switch (category) {
    case "metadata":
      return "Metadata";
    case "skipMarkers":
      return "Skip markers";
    case "subtitles":
      return "Subtitles";
    case "tracking":
      return "Tracking";
    case "acquisition":
      return "Acquisition";
    default:
      return "Other";
  }
}

/**
 * Groups the cards by category, preserving the server's order inside each group.
 *
 * Empty groups are omitted: a heading over nothing is noise on a screen whose only job is to list
 * what exists.
 */
export function groupIntegrations(rows: readonly IntegrationStatus[]): readonly IntegrationGroup[] {
  const groups: IntegrationGroup[] = [];
  for (const category of CATEGORY_ORDER) {
    const items = rows.filter((row) => row.category === category);
    if (items.length > 0) groups.push({ category, label: categoryLabel(category), items });
  }
  return groups;
}

function outcomeDescriptor(outcome: string): StatusDescriptor {
  switch (outcome) {
    case "working":
      return { label: "Working", tone: "ok", detail: null };
    case "rateLimited":
      return {
        label: "Rate limited",
        tone: "warn",
        detail: "The service asked for a pause before the next request.",
      };
    case "unauthorized":
      return {
        label: "Needs attention",
        tone: "danger",
        detail: "The service refused the credential. Check it on the desktop.",
      };
    case "unreachable":
      return {
        label: "Needs attention",
        tone: "danger",
        detail: "The service did not answer. Check the connection on the desktop.",
      };
    case "keychainDenied":
      return {
        label: "Needs attention",
        tone: "danger",
        detail: "The computer's keychain refused the request. Unlock it on the desktop.",
      };
    case "notConfigured":
      return {
        label: "Not configured",
        tone: "neutral",
        detail: "No credential has been stored yet. Add one on the desktop.",
      };
    default:
      // An answer this build does not know is not evidence that everything is fine.
      return { label: "Needs attention", tone: "danger", detail: null };
  }
}

/**
 * The status a card draws.
 *
 * A probe's answer is the most specific fact available, so it wins over the configuration state.
 * Without a probe, a missing required value is "Not configured" and everything else is
 * "Configured"; the server's `needsAttention` covers the missing-key case that is otherwise
 * indistinguishable from a provider that needs no key.
 */
export function describeStatus(status: IntegrationStatus): StatusDescriptor {
  if (status.lastOutcome !== null) return outcomeDescriptor(status.lastOutcome);
  if (!status.configured) {
    return {
      label: "Not configured",
      tone: "neutral",
      detail: status.needsAttention
        ? "A required credential is missing. Add it on the desktop."
        : null,
    };
  }
  if (status.needsAttention) return { label: "Needs attention", tone: "danger", detail: null };
  return { label: "Configured", tone: "neutral", detail: null };
}

/**
 * Whether the card can honestly say it needs no key.
 *
 * The manifest says whether a key is required; the web answer carries that flag, so a keyless
 * source is named rather than inferred from `alwaysOn`. An optional key is not the same claim:
 * the card says it may be added, not that it is needed.
 */
export function credentialFact(status: IntegrationStatus): string | null {
  if (status.requiresKey) return null;
  return status.keyOptional ? "No key required; a key is optional" : "No key required";
}

/** The enabled state a card draws; an always-on source is never "disabled". */
export function enabledLabel(status: IntegrationStatus): string {
  if (status.alwaysOn) return "Always on";
  return status.enabled ? "Enabled" : "Disabled";
}

function IntegrationCard({
  status,
  autofocus,
}: {
  readonly status: IntegrationStatus;
  readonly autofocus: boolean;
}): ReactElement {
  const descriptor = describeStatus(status);
  const credential = credentialFact(status);
  return (
    <article
      className="integration-card"
      data-autofocus={autofocus ? "true" : undefined}
      data-focus="true"
      data-focus-key={"integration:" + status.id}
      tabIndex={0}
    >
      <div className="integration-card-head">
        <h3 className="integration-name">{status.displayName}</h3>
        <span className={"integration-status tone-" + descriptor.tone}>{descriptor.label}</span>
      </div>
      <p className="integration-description">{status.description}</p>
      <div className="integration-facts">
        <span className="integration-fact">{enabledLabel(status)}</span>
        {credential === null ? null : <span className="integration-fact">{credential}</span>}
      </div>
      {descriptor.detail === null ? null : (
        <p className="integration-detail">{descriptor.detail}</p>
      )}
      <p className="integration-manage">Manage credentials on desktop.</p>
    </article>
  );
}

/** The screen without its fetch, so the presentation is testable with fixture data. */
export function IntegrationsView({
  integrations,
  loading,
  error,
  onRetry,
}: {
  readonly integrations: readonly IntegrationStatus[];
  readonly loading: boolean;
  readonly error: string | null;
  readonly onRetry: () => void;
}): ReactElement {
  const groups = groupIntegrations(integrations);
  return (
    <main className="screen integrations-screen">
      <div className="integrations-body">
        <header className="integrations-head">
          <div>
            <h1>Integrations</h1>
            <p className="dim integrations-subtitle">
              The sources this computer can ask. This screen is read-only.
            </p>
          </div>
        </header>
        <p className="integrations-note">
          Turning a source on, ordering it, and managing its credentials happen on the desktop app.
        </p>

        {error === null ? null : (
          <div className="integration-error" role="alert">
            <p>{error}</p>
            <button
              className="secondary"
              data-autofocus="true"
              data-focus="true"
              onClick={onRetry}
              type="button"
            >
              Try again
            </button>
          </div>
        )}

        {loading && integrations.length === 0 ? (
          <p className="dim browse-loading">Reading integrations...</p>
        ) : null}

        {!loading && error === null && integrations.length === 0 ? (
          <p className="integration-empty">No integrations are available in this build.</p>
        ) : null}

        {groups.map((group) => (
          <section
            aria-labelledby={"integration-group-" + group.category}
            className="integration-group"
            key={group.category}
          >
            <h2 id={"integration-group-" + group.category}>{group.label}</h2>
            <div className="integration-grid">
              {group.items.map((status, index) => (
                <IntegrationCard
                  autofocus={error === null && index === 0 && group === groups[0]}
                  key={status.id}
                  status={status}
                />
              ))}
            </div>
          </section>
        ))}
      </div>
    </main>
  );
}

/** The screen the router mounts; the fetch and its retry live here. */
export function IntegrationsScreen(): ReactElement {
  const [integrations, setIntegrations] = useState<IntegrationStatus[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);
  useAutoFocus(integrations[0]?.id ?? error ?? attempt);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setError(null);
    void api
      .integrations()
      .then((found) => {
        if (cancelled) return;
        setIntegrations(found);
      })
      .catch((caught: unknown) => {
        if (cancelled) return;
        setError(caught instanceof Error ? caught.message : "Integrations could not be read.");
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [attempt]);

  return (
    <IntegrationsView
      error={error}
      integrations={integrations}
      loading={loading}
      onRetry={() => setAttempt((current) => current + 1)}
    />
  );
}
