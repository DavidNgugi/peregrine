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
 *
 * **Indexers are managed here, not in Prowlarr's own UI.** The catalog is a summary list and the
 * chosen definition's fields are read on demand, so the payload stays small (AGENTS R11). A field
 * marked secret renders as a password, and a definition that needs no input adds in one click.
 */

import {
  Button,
  Dialog,
  ErrorState,
  SettingsRow,
  SettingsSection,
  TextInput,
} from "@peregrine/ui";
import type { ConfiguredIndexerDto } from "@peregrine/ipc-client";
import type { ReactElement } from "react";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import type { IndexerSettings } from "../hooks/useIndexers.js";

interface IndexersSectionProps {
  /** The configured indexer and the operations this section offers. */
  readonly indexers: IndexerSettings;
}

/** One field of the chosen definition, rendered as the control Prowlarr states. */
function fieldValue(field: { value: string | null; options: { value: string }[] }): string {
  return field.value ?? field.options[0]?.value ?? "";
}

/** The indexer section of the settings screen. */
export function IndexersSection({ indexers }: IndexersSectionProps): ReactElement {
  const { t } = useTranslation();
  const [baseUrl, setBaseUrl] = useState("");
  const [apiKey, setApiKey] = useState("");
  const [confirmingClear, setConfirmingClear] = useState(false);
  const [pendingRemove, setPendingRemove] = useState<ConfiguredIndexerDto | null>(null);
  const [filter, setFilter] = useState("");
  const [chosenKey, setChosenKey] = useState("");
  const [name, setName] = useState("");
  const [values, setValues] = useState<Record<string, string>>({});

  const stored = indexers.status?.baseUrl ?? "";
  // The stored base URL is not a secret, so it fills the field; the key never does.
  useEffect(() => {
    if (stored.length > 0) {
      setBaseUrl(stored);
    }
  }, [stored]);

  // A chosen definition seeds the name and every visible field from the defaults Prowlarr states.
  useEffect(() => {
    const definition = indexers.definition;
    if (definition === null) {
      return;
    }
    setName(definition.name);
    const seeded: Record<string, string> = {};
    for (const field of definition.fields) {
      if (!field.advanced) {
        seeded[field.name] = fieldValue(field);
      }
    }
    setValues(seeded);
  }, [indexers.definition]);

  const configured = indexers.status?.configured ?? false;
  const ready = baseUrl.trim().length > 0 && apiKey.trim().length > 0;
  const needle = filter.trim().toLowerCase();
  const filtered =
    needle.length === 0
      ? indexers.catalog
      : indexers.catalog.filter((entry) => entry.name.toLowerCase().includes(needle));
  const definition = indexers.definition;
  const visibleFields = definition?.fields.filter((field) => !field.advanced) ?? [];

  const outcomes: Record<string, string | undefined> = {
    saved: t("settings.indexers.saved"),
    removed: t("settings.indexers.removed"),
    tested: t("settings.indexers.tested"),
    added: t("settings.indexers.add.added"),
    deleted: t("settings.indexers.deleted"),
  };
  const sentence = indexers.outcome === null ? undefined : outcomes[indexers.outcome];

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

      {sentence === undefined ? null : (
        <p className="px-3 py-2 text-body text-text-secondary" role="status">
          {sentence}
        </p>
      )}
      {indexers.outcome === "testFailed" ? (
        <ErrorState title={t("settings.indexers.testFailed")} />
      ) : null}

      {configured ? (
        <>
          {indexers.indexers.length === 0 ? (
            <SettingsRow label={t("settings.indexers.list.title")}>
              <p className="text-body text-text-secondary">
                {t("settings.indexers.list.empty")}
              </p>
            </SettingsRow>
          ) : (
            indexers.indexers.map((row) => (
              <SettingsRow key={row.id} label={row.name}>
                <div className="flex justify-end">
                  <Button
                    disabled={indexers.busy}
                    onClick={() => {
                      setPendingRemove(row);
                    }}
                    size="sm"
                    variant="danger"
                  >
                    {t("settings.indexers.remove")}
                  </Button>
                </div>
              </SettingsRow>
            ))
          )}

          <SettingsRow label={t("settings.indexers.add.title")}>
            <div className="flex flex-col items-end gap-2">
              <div className="flex items-center gap-2">
                <TextInput
                  className="w-48"
                  label={t("settings.indexers.add.search")}
                  labelHidden
                  onChange={(event) => {
                    setFilter(event.target.value);
                  }}
                  placeholder={t("settings.indexers.add.searchPlaceholder")}
                  value={filter}
                />
                <Button
                  disabled={indexers.busy}
                  onClick={() => {
                    void indexers.loadCatalog();
                  }}
                  size="sm"
                  variant="secondary"
                >
                  {t("settings.indexers.add.browse")}
                </Button>
              </div>
              {indexers.catalog.length === 0 ? null : (
                <select
                  aria-label={t("settings.indexers.add.choose")}
                  className="h-8 w-72 rounded-md border border-border-subtle bg-surface-2 px-2 text-body text-text-primary"
                  onChange={(event) => {
                    setChosenKey(event.target.value);
                    void indexers.choose(event.target.value);
                  }}
                  value={chosenKey}
                >
                  <option value="">{t("settings.indexers.add.choose")}</option>
                  {filtered.map((entry) => (
                    <option key={entry.key} value={entry.key}>
                      {entry.name}
                    </option>
                  ))}
                </select>
              )}
            </div>
          </SettingsRow>

          {definition === null ? null : (
            <>
              <SettingsRow label={t("settings.indexers.add.name")}>
                <TextInput
                  className="w-72"
                  label={t("settings.indexers.add.name")}
                  labelHidden
                  onChange={(event) => {
                    setName(event.target.value);
                  }}
                  value={name}
                />
              </SettingsRow>
              {visibleFields.map((field) => (
                <SettingsRow key={field.name} label={field.label}>
                  {field.options.length > 0 ? (
                    <select
                      aria-label={field.label}
                      className="h-8 w-72 rounded-md border border-border-subtle bg-surface-2 px-2 text-body text-text-primary"
                      onChange={(event) => {
                        setValues((previous) => ({
                          ...previous,
                          [field.name]: event.target.value,
                        }));
                      }}
                      value={values[field.name] ?? ""}
                    >
                      {field.options.map((option) => (
                        <option key={option.value} value={option.value}>
                          {option.label}
                        </option>
                      ))}
                    </select>
                  ) : field.kind === "checkbox" ? (
                    <input
                      aria-label={field.label}
                      checked={(values[field.name] ?? "") === "true"}
                      onChange={(event) => {
                        setValues((previous) => ({
                          ...previous,
                          [field.name]: event.target.checked ? "true" : "false",
                        }));
                      }}
                      type="checkbox"
                    />
                  ) : (
                    <TextInput
                      className="w-72"
                      label={field.label}
                      labelHidden
                      onChange={(event) => {
                        setValues((previous) => ({
                          ...previous,
                          [field.name]: event.target.value,
                        }));
                      }}
                      spellCheck={false}
                      type={field.secret ? "password" : "text"}
                      value={values[field.name] ?? ""}
                    />
                  )}
                </SettingsRow>
              ))}
              <SettingsRow label="">
                <div className="flex justify-end">
                  <Button
                    disabled={indexers.busy || name.trim().length === 0}
                    onClick={() => {
                      void indexers.add({
                        key: definition.key,
                        name: name.trim(),
                        fields: visibleFields.map((field) => ({
                          name: field.name,
                          value: values[field.name] ?? "",
                        })),
                      });
                    }}
                    size="sm"
                    variant="primary"
                  >
                    {t("settings.indexers.add.submit")}
                  </Button>
                </div>
              </SettingsRow>
            </>
          )}
        </>
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
        <div className="flex items-center justify-end gap-2">
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
                setConfirmingClear(true);
              }}
              size="sm"
              variant="danger"
            >
              {t("settings.indexers.clear")}
            </Button>
          ) : null}
        </div>
      </SettingsRow>

      {confirmingClear ? (
        <Dialog
          closeLabel={t("common.close")}
          description={t("settings.indexers.confirm.body")}
          onClose={() => {
            setConfirmingClear(false);
          }}
          title={t("settings.indexers.confirm.title")}
        >
          <div className="flex justify-end gap-2">
            <Button
              onClick={() => {
                setConfirmingClear(false);
              }}
              size="sm"
              variant="ghost"
            >
              {t("common.cancel")}
            </Button>
            <Button
              onClick={() => {
                setConfirmingClear(false);
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

      {pendingRemove === null ? null : (
        <Dialog
          closeLabel={t("common.close")}
          description={t("settings.indexers.removeConfirm.body", { name: pendingRemove.name })}
          onClose={() => {
            setPendingRemove(null);
          }}
          title={t("settings.indexers.removeConfirm.title")}
        >
          <div className="flex justify-end gap-2">
            <Button
              onClick={() => {
                setPendingRemove(null);
              }}
              size="sm"
              variant="ghost"
            >
              {t("common.cancel")}
            </Button>
            <Button
              onClick={() => {
                const target = pendingRemove;
                setPendingRemove(null);
                void indexers.remove(target.id);
              }}
              size="sm"
              variant="danger"
            >
              {t("settings.indexers.remove")}
            </Button>
          </div>
        </Dialog>
      )}
    </SettingsSection>
  );
}
