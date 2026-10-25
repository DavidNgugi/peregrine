/**
 * Settings -> Integrations: every service this build can ask, grouped by what it is for.
 *
 * **The core owns the state.** `configured`, `lastOutcome`, and the credential fields are the
 * DTO's answers; this file groups them by category, draws them in the manifest order the core
 * sent, and turns a click into one of the commands. The only decision that is not a rendering
 * one is `needsAttention`, and that rule is named in `useIntegrations.ts` where it can be read.
 *
 * **The card is a projection, not a form with a memory.** Nothing here renders a stored value:
 * `field.present` decides whether the action says Replace or Save, and the candidate is handed
 * to the command and cleared. A removal is confirmed with `Dialog` because §11 rule 7 bans
 * `window.confirm`, and every control is a native button, input, or switch so the keyboard path
 * is the one the browser already has.
 *
 * Copy defaults are inline because the English catalogue is edited by the parent change; each
 * key is also listed in the hand-off so the catalogue can take the value over.
 */

import {
  Button,
  Dialog,
  ErrorState,
  SettingsRow,
  SettingsSection,
  Switch,
  TextInput,
  Toast,
} from "@peregrine/ui";
import type { ReactElement } from "react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import {
  integrationStateKey,
  lastOutcomeKey,
  needsAttention,
  priorityMove,
  type CredentialValue,
  type IntegrationDto,
  type IntegrationSettings,
} from "../hooks/useIntegrations.js";

/** The categories in the order the section draws them, which is not the manifest's order. */
const CATEGORY_ORDER: readonly IntegrationDto["category"][] = [
  "metadata",
  "skipMarkers",
  "subtitles",
  "tracking",
  "acquisition",
];

/**
 * The English sentence for each key this surface uses.
 *
 * The catalogue is the source once the keys land; until then these are what a static render
 * shows, and what the tests assert. A key missing from this table falls back to the key itself,
 * which is a visible failure rather than a blank line.
 */
const COPY: Readonly<Record<string, string>> = {
  "settings.integrations.loading": "Reading the integrations…",
  "settings.integrations.empty": "No integration is built into this build.",
  "settings.integrations.retry": "Try again",
  "settings.integrations.unavailable.title": "Your integrations could not be read.",
  "settings.integrations.unavailable.body":
    "Peregrine's core did not answer, so this list is unknown rather than empty. Reopening Settings tries the read again.",
  "settings.integrations.configured": "Configured",
  "settings.integrations.notConfigured": "Not configured",
  "settings.integrations.store.unread": "Checking the keychain…",
  "settings.integrations.store.denied": "Keychain access refused",
  "settings.integrations.store.unavailable": "No keychain available",
  "settings.integrations.noKeyRequired": "No key required",
  "settings.integrations.keyOptional": "Key optional",
  "settings.integrations.alwaysOn": "Always on",
  "settings.integrations.needsAttention": "Needs attention",
  "settings.integrations.lastOutcome.working": "Last test worked",
  "settings.integrations.lastOutcome.unauthorized": "Last test was refused",
  "settings.integrations.lastOutcome.rateLimited": "Last test was rate limited",
  "settings.integrations.lastOutcome.unreachable": "Last test could not reach the service",
  "settings.integrations.lastOutcome.keychainDenied": "The keychain refused the last test",
  "settings.integrations.lastOutcome.notConfigured": "The last test found no credential",
  "settings.integrations.capabilities": "Capabilities",
  "settings.integrations.capability.metadata": "Metadata",
  "settings.integrations.capability.markers": "Skip markers",
  "settings.integrations.capability.subtitles": "Subtitles",
  "settings.integrations.capability.scrobble": "Scrobbling",
  "settings.integrations.capability.acquisition": "Acquisition",
  "settings.integrations.category.metadata": "Metadata",
  "settings.integrations.category.skipMarkers": "Skip markers",
  "settings.integrations.category.subtitles": "Subtitles",
  "settings.integrations.category.tracking": "Tracking",
  "settings.integrations.category.acquisition": "Acquisition",
  "settings.integrations.docs": "Documentation",
  "settings.integrations.docsNamed": "{{name}} documentation",
  "settings.integrations.enabled": "Enabled for {{name}}",
  "settings.integrations.enabledDescription":
    "Turned off, this source is never asked. It stays configured.",
  "settings.integrations.priority": "Priority",
  "settings.integrations.priorityDescription":
    "Installation-wide. A source earlier in its category is asked first.",
  "settings.integrations.moveUp": "Up",
  "settings.integrations.moveDown": "Down",
  "settings.integrations.moveUpNamed": "Move {{name}} up",
  "settings.integrations.moveDownNamed": "Move {{name}} down",
  "settings.integrations.submission": "Allow submissions from {{name}}",
  "settings.integrations.submissionDescription":
    "Sends the timestamps you contribute back to this source.",
  "settings.integrations.test": "Test connection",
  "settings.integrations.remove": "Remove credentials",
  "settings.integrations.removeNamed": "Remove the credentials for {{name}}",
  "settings.integrations.save": "Save",
  "settings.integrations.replace": "Replace",
  "settings.integrations.fieldDescription":
    "Stored in your operating system's keychain. It is never shown again after saving.",
  "settings.integrations.confirm.title": "Remove the {{name}} credentials?",
  "settings.integrations.confirm.body":
    "Peregrine forgets the stored values and stops asking this source with them. Nothing else is removed.",
  "settings.integrations.outcome.tested": "The connection to {{name}} works.",
  "settings.integrations.outcome.saved": "The credentials were stored for {{name}}.",
  "settings.integrations.outcome.removed":
    "The credentials were removed. {{name}} is no longer asked with them.",
  "settings.integrations.outcome.unauthorized":
    "That credential was refused. Check you copied the right value.",
  "settings.integrations.outcome.rateLimited":
    "{{name}} is rate limiting this app; try again in a minute.",
  "settings.integrations.outcome.unreachable": "{{name}} could not be reached.",
  "settings.integrations.outcome.notConfigured":
    "No credential is stored for {{name}}, so there was nothing to test.",
  "settings.integrations.outcome.store":
    "Peregrine could not read its credential store, so nothing was saved.",
  "settings.integrations.outcome.cancelled": "That was cancelled. Nothing was changed.",
  "settings.integrations.outcome.failed":
    "The credential could not be confirmed. Try again, and check the service's status if it keeps happening.",
  "settings.integrations.pending.tested": "Testing {{name}}…",
  "settings.integrations.pending.saved": "Saving credentials for {{name}}…",
  "settings.integrations.pending.removed": "Removing credentials for {{name}}…",
  "common.dismiss": "Dismiss",
  // Already in the catalogue; the defaults keep this table total rather than special-casing them.
  "common.cancel": "Cancel",
  "common.close": "Close",
};

/**
 * The translated sentence for one key, with the key's inline English as its fallback.
 *
 * One interpolation value is enough for this surface: every sentence that names anything names
 * the integration. Keeping it to `name` also keeps the call sites readable.
 */
function useCopy(): (key: string, name?: string) => string {
  const { t } = useTranslation();
  return (key, name) =>
    name === undefined
      ? t(key, { defaultValue: COPY[key] ?? key })
      : t(key, { defaultValue: COPY[key] ?? key, name });
}

interface IntegrationsSectionProps {
  /** The list and the actions the settings screen offers. */
  readonly integrations: IntegrationSettings;
}

/** The integrations section of the settings screen. */
export function IntegrationsSection({ integrations }: IntegrationsSectionProps): ReactElement {
  const say = useCopy();
  // One dismissal per result. A new result changes the key and is shown again, which is what makes
  // the toast a report rather than a permanent banner.
  const [dismissed, setDismissed] = useState<string | null>(null);
  const outcome = integrations.outcome;
  const pending = integrations.pending;
  const named =
    outcome === null
      ? null
      : integrations.integrations.find((entry) => entry.id === outcome.integration);
  const pendingName =
    pending === null
      ? pending
      : (integrations.integrations.find((entry) => entry.id === pending.integration)?.displayName ??
        pending.integration);
  const toastKey =
    pending !== null
      ? `pending:${pending.integration}:${pending.action}`
      : integrations.error !== null
        ? `error:${integrations.error}`
        : outcome !== null
          ? `outcome:${outcome.integration}:${outcome.key}:${String(outcome.ok)}`
          : null;
  const dismissal = (): void => {
    if (toastKey !== null) {
      setDismissed(toastKey);
    }
  };

  return (
    <>
      {integrations.failed ? (
        <ErrorState
          action={
            <Button
              onClick={() => {
                integrations.reload();
              }}
              size="sm"
              variant="secondary"
            >
              {say("settings.integrations.retry")}
            </Button>
          }
          body={say("settings.integrations.unavailable.body")}
          title={say("settings.integrations.unavailable.title")}
        />
      ) : null}

      {toastKey === null || dismissed === toastKey ? null : pending !== null ? (
        <Toast
          dismissLabel={say("common.dismiss")}
          onDismiss={dismissal}
          title={say(
            `settings.integrations.pending.${pending.action}`,
            pendingName ?? pending.integration,
          )}
          tone="progress"
        />
      ) : integrations.error !== null ? (
        <Toast
          autoDismissMs={10_000}
          dismissLabel={say("common.dismiss")}
          onDismiss={dismissal}
          title={integrations.error}
          tone="danger"
        />
      ) : outcome !== null ? (
        <Toast
          autoDismissMs={outcome.ok ? 5_000 : 10_000}
          dismissLabel={say("common.dismiss")}
          onDismiss={dismissal}
          title={say(outcome.key, named?.displayName ?? outcome.integration)}
          tone={outcome.ok ? "success" : "danger"}
        />
      ) : null}

      {integrations.loading ? (
        <p className="px-3 py-2 text-body text-text-secondary" role="status">
          {say("settings.integrations.loading")}
        </p>
      ) : null}

      {!integrations.loading && !integrations.failed && integrations.integrations.length === 0 ? (
        <p className="px-3 py-2 text-body text-text-secondary">
          {say("settings.integrations.empty")}
        </p>
      ) : null}

      {integrations.loading || integrations.failed
        ? null
        : CATEGORY_ORDER.map((category) => {
            const entries = integrations.integrations
              .filter((entry) => entry.category === category)
              .sort((left, right) => left.priority - right.priority);
            if (entries.length === 0) {
              return null;
            }
            return (
              <SettingsSection
                key={category}
                title={say(`settings.integrations.category.${category}`)}
              >
                {entries.map((entry) => (
                  <IntegrationCard entry={entry} key={entry.id} settings={integrations} />
                ))}
              </SettingsSection>
            );
          })}
    </>
  );
}

interface IntegrationCardProps {
  readonly entry: IntegrationDto;
  readonly settings: IntegrationSettings;
}

/** One integration: what it is, what it can do, and the controls that change it. */
function IntegrationCard({ entry, settings }: IntegrationCardProps): ReactElement {
  const say = useCopy();
  const [confirming, setConfirming] = useState(false);
  const attention = needsAttention(entry);
  const removable = entry.fields.some((field) => field.present);
  const canMoveUp = priorityMove(settings.integrations, entry.id, "up") !== null;
  const canMoveDown = priorityMove(settings.integrations, entry.id, "down") !== null;

  return (
    <div
      className={["flex flex-col gap-2 py-3", attention ? "border-l-2 border-state-warn" : ""].join(
        " ",
      )}
      data-needs-attention={attention ? "true" : "false"}
    >
      <div className="flex items-start justify-between gap-4 px-3">
        <div className="flex flex-col gap-1">
          <h3 className="text-body-strong text-text-primary">{entry.displayName}</h3>
          <p className="text-caption text-text-tertiary">{entry.description}</p>
        </div>
        <a
          aria-label={say("settings.integrations.docsNamed", entry.displayName)}
          className="shrink-0 rounded-sm text-caption text-text-primary underline-offset-2 hover:underline focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-focus-ring"
          href={entry.docs}
          rel="noreferrer noopener"
          target="_blank"
        >
          {say("settings.integrations.docs")}
        </a>
      </div>

      <div className="flex flex-wrap items-center gap-x-3 gap-y-1 px-3 text-caption text-text-secondary">
        <span>{say(integrationStateKey(entry.configured, entry.store))}</span>
        {entry.lastOutcome === null ? null : <span>{say(lastOutcomeKey(entry.lastOutcome))}</span>}
        {!entry.requiresKey && entry.fields.length === 0 ? (
          <span>{say("settings.integrations.noKeyRequired")}</span>
        ) : null}
        {entry.keyOptional ? <span>{say("settings.integrations.keyOptional")}</span> : null}
        {attention ? (
          <span className="text-state-warn">{say("settings.integrations.needsAttention")}</span>
        ) : null}
      </div>

      <p className="px-3 text-caption text-text-tertiary">
        {say("settings.integrations.capabilities")}:{" "}
        {entry.capabilities
          .map((capability) => say(`settings.integrations.capability.${capability}`))
          .join(" · ")}
      </p>

      {entry.alwaysOn ? (
        <SettingsRow
          description={say("settings.integrations.enabledDescription")}
          label={say("settings.integrations.enabled", entry.displayName)}
        >
          <span className="text-caption text-text-secondary">
            {say("settings.integrations.alwaysOn")}
          </span>
        </SettingsRow>
      ) : (
        <Switch
          busy={settings.busy}
          checked={entry.enabled}
          description={say("settings.integrations.enabledDescription")}
          label={say("settings.integrations.enabled", entry.displayName)}
          onChange={(next) => {
            void settings.setEnabled(entry.id, next);
          }}
        />
      )}

      <SettingsRow
        description={say("settings.integrations.priorityDescription")}
        label={say("settings.integrations.priority")}
      >
        <div className="flex items-center gap-1">
          <Button
            aria-label={say("settings.integrations.moveUpNamed", entry.displayName)}
            disabled={settings.busy || !canMoveUp}
            onClick={() => {
              void settings.move(entry.id, "up");
            }}
            size="sm"
            variant="secondary"
          >
            {say("settings.integrations.moveUp")}
          </Button>
          <Button
            aria-label={say("settings.integrations.moveDownNamed", entry.displayName)}
            disabled={settings.busy || !canMoveDown}
            onClick={() => {
              void settings.move(entry.id, "down");
            }}
            size="sm"
            variant="secondary"
          >
            {say("settings.integrations.moveDown")}
          </Button>
        </div>
      </SettingsRow>

      {entry.submissionSupport ? (
        <Switch
          busy={settings.busy}
          checked={entry.submissionEnabled}
          description={say("settings.integrations.submissionDescription")}
          label={say("settings.integrations.submission", entry.displayName)}
          onChange={(next) => {
            void settings.setSubmission(entry.id, next);
          }}
        />
      ) : null}

      {entry.fields.map((field) => (
        <CredentialField
          busy={settings.busy}
          field={field}
          integration={entry.id}
          key={field.name}
          onSave={settings.saveCredentials}
        />
      ))}

      {entry.alwaysOn && !removable ? null : (
        <div className="flex flex-wrap items-center gap-2 px-3">
          {entry.alwaysOn ? null : (
            <Button
              disabled={settings.busy}
              onClick={() => {
                void settings.testConnection(entry.id);
              }}
              size="sm"
              variant="secondary"
            >
              {say("settings.integrations.test")}
            </Button>
          )}
          {removable ? (
            <Button
              aria-label={say("settings.integrations.removeNamed", entry.displayName)}
              disabled={settings.busy}
              onClick={() => {
                setConfirming(true);
              }}
              size="sm"
              variant="danger"
            >
              {say("settings.integrations.remove")}
            </Button>
          ) : null}
        </div>
      )}

      {confirming ? (
        <Dialog
          closeLabel={say("common.close")}
          description={say("settings.integrations.confirm.body")}
          onClose={() => {
            setConfirming(false);
          }}
          title={say("settings.integrations.confirm.title", entry.displayName)}
        >
          <div className="flex justify-end gap-2">
            <Button
              onClick={() => {
                setConfirming(false);
              }}
              size="sm"
              variant="ghost"
            >
              {say("common.cancel")}
            </Button>
            <Button
              onClick={() =>
                settings.removeCredentials(entry.id).finally(() => {
                  setConfirming(false);
                })
              }
              size="sm"
              variant="danger"
            >
              {say("settings.integrations.remove")}
            </Button>
          </div>
        </Dialog>
      ) : null}
    </div>
  );
}

interface CredentialFieldProps {
  readonly busy: boolean;
  readonly field: IntegrationDto["fields"][number];
  readonly integration: string;
  readonly onSave: (id: string, values: readonly CredentialValue[]) => Promise<void>;
}

/** One write-only credential field, with the action that stores this candidate. */
function CredentialField({ busy, field, integration, onSave }: CredentialFieldProps): ReactElement {
  const say = useCopy();
  const [value, setValue] = useState("");
  const empty = value.trim().length === 0;

  return (
    <SettingsRow description={say("settings.integrations.fieldDescription")} label={field.label}>
      <div className="flex items-center gap-2">
        <TextInput
          autoComplete="off"
          className="w-64"
          label={field.label}
          labelHidden
          onChange={(event) => {
            setValue(event.target.value);
          }}
          spellCheck={false}
          type="password"
          value={value}
        />
        <Button
          disabled={busy || empty}
          onClick={() => {
            // The candidate moves into the call and out of the field: a value left in a form is a
            // value left on screen after the reason for having it has gone.
            const candidate = value.trim();
            setValue("");
            return onSave(integration, [{ field: field.name, value: candidate }]);
          }}
          size="sm"
          variant="primary"
        >
          {field.present ? say("settings.integrations.replace") : say("settings.integrations.save")}
        </Button>
      </div>
    </SettingsRow>
  );
}
