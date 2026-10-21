/**
 * The metadata providers, and the two things a person does about a key.
 *
 * Every element is the kit's — `SettingsSection` owns the group, `SettingsRow` a row, `TextInput`
 * the field, `Button` the actions, and `Dialog` the confirmation — so this file holds copy and
 * state and no control of its own (PG-E7-03, `DESIGN_SYSTEM` §11 rule 7 bans `window.confirm`).
 *
 * **The key is written as a password field and never read back.** Nothing here renders what the
 * core stored: `configured` is a flag, and the field is for a candidate the user pastes. The
 * candidate is dropped from the field as soon as it is handed over, so it does not linger in the
 * component either.
 *
 * **No rule about a provider is computed here.** Whether a provider can be used is the core's
 * answer (`configured`), which is what decides whether the actions offered make sense; the screen
 * reports what the core says a finished job did rather than deciding it.
 */

import {
  Button,
  Dialog,
  EmptyState,
  ErrorState,
  SettingsRow,
  SettingsSection,
  TextInput,
} from "@peregrine/ui";
import type { CredentialStoreDto, ProviderDto } from "@peregrine/ipc-client";
import type { ReactElement } from "react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { providerStateKey, storeNoticeKey } from "../provider-outcome.js";
import type { ProviderSettings } from "../hooks/useProviders.js";

interface ProvidersSectionProps {
  /** The known providers and the operations this section offers. */
  readonly providers: ProviderSettings;
}

/** What is known about one provider, as the line under its name. */
function describe(entry: ProviderDto, state: string): string {
  return `${entry.endpoint} · ${state}`;
}

interface ProviderRowProps {
  readonly busy: boolean;
  readonly entry: ProviderDto;
  /**
   * What the last read of the keychain answered.
   *
   * The row needs it to avoid claiming "No key stored" when the truth is that nobody could look.
   */
  readonly store: CredentialStoreDto;
  readonly onRemove: (provider: string) => Promise<void>;
  readonly onSave: (provider: string, key: string) => Promise<void>;
  readonly onTest: (provider: string) => Promise<void>;
}

/** One provider's facts, its key field, and the actions that make sense for its state. */
function ProviderRow({
  busy,
  entry,
  onRemove,
  onSave,
  onTest,
  store,
}: ProviderRowProps): ReactElement {
  const { t } = useTranslation();
  const [key, setKey] = useState("");
  const [confirming, setConfirming] = useState(false);

  return (
    <>
      <SettingsRow
        description={describe(entry, t(providerStateKey(entry.configured, store)))}
        label={entry.displayName}
      >
        {entry.configured ? (
          <div className="flex items-center gap-2">
            <Button
              disabled={busy}
              onClick={() => onTest(entry.provider)}
              size="sm"
              variant="secondary"
            >
              {t("settings.providers.key.test")}
            </Button>
            <Button
              disabled={busy}
              onClick={() => {
                setConfirming(true);
              }}
              size="sm"
              variant="danger"
            >
              {t("settings.providers.remove")}
            </Button>
          </div>
        ) : null}
      </SettingsRow>

      <SettingsRow
        description={t("settings.providers.key.description", { provider: entry.displayName })}
        label={t("settings.providers.key.label")}
      >
        <div className="flex items-center gap-2">
          <TextInput
            autoComplete="off"
            className="w-64"
            label={t("settings.providers.key.accessible", { provider: entry.displayName })}
            labelHidden
            onChange={(event) => {
              setKey(event.target.value);
            }}
            placeholder={t("settings.providers.key.placeholder")}
            spellCheck={false}
            type="password"
            value={key}
          />
          <Button
            disabled={busy}
            onClick={() => {
              // The candidate moves into the call and out of the field: a key left in a form is a
              // key left on screen after the reason for having it has gone.
              const candidate = key;
              setKey("");
              return onSave(entry.provider, candidate);
            }}
            size="sm"
            variant="primary"
          >
            {t("settings.providers.key.save")}
          </Button>
        </div>
      </SettingsRow>

      {confirming ? (
        <Dialog
          closeLabel={t("common.close")}
          description={t("settings.providers.confirm.body")}
          onClose={() => {
            setConfirming(false);
          }}
          title={t("settings.providers.confirm.title", { provider: entry.displayName })}
        >
          <div className="flex justify-end gap-2">
            <Button
              onClick={() => {
                setConfirming(false);
              }}
              size="sm"
              variant="ghost"
            >
              {t("common.cancel")}
            </Button>
            <Button
              onClick={() => {
                setConfirming(false);
                void onRemove(entry.provider);
              }}
              size="sm"
              variant="danger"
            >
              {t("settings.providers.remove")}
            </Button>
          </div>
        </Dialog>
      ) : null}
    </>
  );
}

/** The metadata providers section of the settings screen. */
export function ProvidersSection({ providers }: ProvidersSectionProps): ReactElement {
  const { t } = useTranslation();
  const outcome = providers.outcome;
  const named =
    outcome === null
      ? null
      : providers.providers.find((entry) => entry.provider === outcome.provider);
  const notice = storeNoticeKey(providers.store);

  return (
    <SettingsSection
      description={t("settings.providers.description")}
      title={t("settings.providers.title")}
    >
      {providers.failed ? (
        <ErrorState
          body={t("settings.providers.unavailable.body")}
          title={t("settings.providers.unavailable.title")}
        />
      ) : null}

      {providers.error !== null ? <ErrorState title={providers.error} /> : null}

      {/* One sentence about the keychain itself, when it did not simply answer. It is what stops the
          rows below saying "No key stored" to someone whose key is still in their keychain — the
          reading a reinstall deleted it. */}
      {notice === null ? null : (
        <div className="flex flex-col gap-2 px-3 py-2">
          <p className="text-body text-text-secondary" role="status">
            {t(notice)}
          </p>
          {/* Offered whenever the keychain has not answered, including after the bounded poll has
              stopped: a person who has just allowed the request needs a way to ask again without
              reopening the app. */}
          <div>
            <Button
              disabled={providers.busy}
              onClick={() => {
                providers.reload();
              }}
              size="sm"
              variant="secondary"
            >
              {t("settings.providers.retry")}
            </Button>
          </div>
        </div>
      )}

      {providers.busy && outcome === null ? (
        <p className="px-3 py-2 text-body text-text-secondary" role="status">
          {t("settings.providers.checking")}
        </p>
      ) : null}

      {outcome !== null ? (
        outcome.ok ? (
          <p className="px-3 py-2 text-body text-text-secondary" role="status">
            {t(outcome.key, { provider: named?.displayName ?? outcome.provider })}
          </p>
        ) : (
          <ErrorState
            title={t(outcome.key, { provider: named?.displayName ?? outcome.provider })}
          />
        )
      ) : null}

      {providers.providers.map((entry) => (
        <ProviderRow
          busy={providers.busy}
          entry={entry}
          key={entry.provider}
          onRemove={providers.removeKey}
          onSave={providers.saveKey}
          onTest={providers.testConnection}
          store={providers.store}
        />
      ))}

      {!providers.loading && !providers.failed && providers.providers.length === 0 ? (
        <EmptyState
          body={t("settings.providers.empty.body")}
          size="inline"
          title={t("settings.providers.empty.title")}
        />
      ) : null}
    </SettingsSection>
  );
}
