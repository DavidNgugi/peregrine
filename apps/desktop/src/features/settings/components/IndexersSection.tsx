/**
 * The indexer section of the settings screen.
 *
 * Prowlarr is the one external service this build asks to find releases. It is configured here
 * rather than discovered, and every element is the kit's — `SettingsSection` owns the group,
 * `SettingsRow` a row, `TextInput` the fields, `Button` the actions, and `Dialog` the
 * confirmation (`DESIGN_SYSTEM` §11 bans `window.confirm`).
 *
 * **The key is a password field and is never read back.** `configured` is a flag and the core
 * answers the base URL, never the key. Save and Test both need a key in the field because the
 * command proves the candidate rather than the stored row, so testing an already-configured
 * indexer asks for the key again; the base URL is prefilled because it is not a secret.
 */

import {
  Button,
  Dialog,
  ErrorState,
  SettingsRow,
  SettingsSection,
  TextInput,
} from "@peregrine/ui";
import type { ReactElement } from "react";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import type { IndexerSettings } from "../hooks/useIndexers.js";

interface IndexersSectionProps {
  /** The configured indexer and the operations this section offers. */
  readonly indexers: IndexerSettings;
}

/** The indexer section of the settings screen. */
export function IndexersSection({ indexers }: IndexersSectionProps): ReactElement {
  const { t } = useTranslation();
  const [baseUrl, setBaseUrl] = useState("");
  const [apiKey, setApiKey] = useState("");
  const [confirming, setConfirming] = useState(false);

  const stored = indexers.status?.baseUrl ?? "";
  // The stored base URL is not a secret, so it fills the field; the key never does.
  useEffect(() => {
    if (stored.length > 0) {
      setBaseUrl(stored);
    }
  }, [stored]);

  const configured = indexers.status?.configured ?? false;
  const ready = baseUrl.trim().length > 0 && apiKey.trim().length > 0;

  return (
    <SettingsSection
      description={t("settings.indexers.description")}
      title={t("settings.indexers.title")}
    >
      {indexers.failed ? (
        <ErrorState
          body={t("settings.indexers.unavailable.body")}
          title={t("settings.indexers.unavailable.title")}
        />
      ) : null}

      {indexers.error !== null ? <ErrorState title={indexers.error} /> : null}

      {indexers.outcome === "saved" ? (
        <p className="px-3 py-2 text-body text-text-secondary" role="status">
          {t("settings.indexers.saved")}
        </p>
      ) : null}
      {indexers.outcome === "removed" ? (
        <p className="px-3 py-2 text-body text-text-secondary" role="status">
          {t("settings.indexers.removed")}
        </p>
      ) : null}
      {indexers.outcome === "tested" ? (
        <p className="px-3 py-2 text-body text-text-secondary" role="status">
          {t("settings.indexers.tested")}
        </p>
      ) : null}
      {indexers.outcome === "testFailed" ? (
        <ErrorState title={t("settings.indexers.testFailed")} />
      ) : null}

      <SettingsRow
        description={t("settings.indexers.baseUrl.description")}
        label={t("settings.indexers.baseUrl.label")}
      >
        <TextInput
          autoComplete="off"
          className="w-72"
          label={t("settings.indexers.baseUrl.accessible")}
          labelHidden
          onChange={(event) => {
            setBaseUrl(event.target.value);
          }}
          placeholder={t("settings.indexers.baseUrl.placeholder")}
          spellCheck={false}
          type="url"
          value={baseUrl}
        />
      </SettingsRow>

      <SettingsRow
        description={t("settings.indexers.key.description")}
        label={t("settings.indexers.key.label")}
      >
        <TextInput
          autoComplete="off"
          className="w-72"
          label={t("settings.indexers.key.accessible")}
          labelHidden
          onChange={(event) => {
            setApiKey(event.target.value);
          }}
          placeholder={t("settings.indexers.key.placeholder")}
          spellCheck={false}
          type="password"
          value={apiKey}
        />
      </SettingsRow>

      <SettingsRow
        description={
          configured
            ? t("settings.indexers.configured", { url: stored })
            : t("settings.indexers.notConfigured")
        }
        label={t("settings.indexers.status.label")}
      >
        <div className="flex items-center gap-2">
          <Button
            disabled={!ready || indexers.busy}
            onClick={() => {
              // The candidate moves into the call and out of the field, so a key does not linger
              // on screen after the reason for having it has gone.
              const candidate = apiKey;
              setApiKey("");
              void indexers.save(baseUrl.trim(), candidate);
            }}
            size="sm"
            variant="primary"
          >
            {t("settings.indexers.save")}
          </Button>
          <Button
            disabled={!ready || indexers.busy}
            onClick={() => {
              void indexers.test(baseUrl.trim(), apiKey);
            }}
            size="sm"
            variant="secondary"
          >
            {t("settings.indexers.test")}
          </Button>
          {configured ? (
            <Button
              disabled={indexers.busy}
              onClick={() => {
                setConfirming(true);
              }}
              size="sm"
              variant="danger"
            >
              {t("settings.indexers.clear")}
            </Button>
          ) : null}
        </div>
      </SettingsRow>

      {confirming ? (
        <Dialog
          closeLabel={t("common.close")}
          description={t("settings.indexers.confirm.body")}
          onClose={() => {
            setConfirming(false);
          }}
          title={t("settings.indexers.confirm.title")}
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
                void indexers.clear();
              }}
              size="sm"
              variant="danger"
            >
              {t("settings.indexers.clear")}
            </Button>
          </div>
        </Dialog>
      ) : null}
    </SettingsSection>
  );
}
