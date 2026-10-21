/**
 * Settings -> Usenet: the providers a transfer may use, and the archive policy.
 *
 * The password is the one field that is never read back. Editing a provider leaves it blank, and a
 * blank password on save keeps the stored one.
 */

import { Button, ErrorState, SettingsRow, SettingsSection, Switch, TextInput } from "@peregrine/ui";
import type { ReactElement } from "react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import type { UsenetServerInputDto } from "@peregrine/ipc-client";
import type { UsenetSettings } from "../hooks/useUsenet.js";

interface FormState {
  id: number | null;
  name: string;
  host: string;
  port: string;
  tls: boolean;
  username: string;
  password: string;
  connections: string;
  priority: string;
  enabled: boolean;
}

const BLANK: FormState = {
  id: null,
  name: "",
  host: "",
  port: "563",
  tls: true,
  username: "",
  password: "",
  connections: "8",
  priority: "100",
  enabled: true,
};

interface UsenetSectionProps {
  readonly usenet: UsenetSettings;
}

export function UsenetSection({ usenet }: UsenetSectionProps): ReactElement {
  const { t } = useTranslation();
  const [form, setForm] = useState<FormState>(BLANK);

  const update = (patch: Partial<FormState>): void => {
    setForm((current) => ({ ...current, ...patch }));
  };

  const submit = async (): Promise<void> => {
    const input: UsenetServerInputDto = {
      id: form.id,
      name: form.name,
      host: form.host,
      port: Number(form.port),
      tls: form.tls,
      username: form.username,
      password: form.password === "" ? null : form.password,
      connections: Number(form.connections),
      priority: Number(form.priority),
      enabled: form.enabled,
    };
    await usenet.save(input);
    setForm((current) => ({ ...current, password: "" }));
  };

  return (
    <SettingsSection
      description={t("settings.usenet.description")}
      title={t("settings.usenet.title")}
    >
      {usenet.loading ? null : usenet.failed ? (
        <ErrorState body={t("settings.usenet.failed")} title={t("settings.usenet.failedTitle")} />
      ) : (
        <>
          {usenet.status?.servers.length === 0 ? (
            <p className="text-body text-text-tertiary">{t("settings.usenet.empty")}</p>
          ) : (
            usenet.status?.servers.map((server) => (
              <SettingsRow
                className="usenet-server-row"
                key={server.id}
                label={`${server.name} — ${server.host}:${String(server.port)}`}
              >
                <div className="usenet-server-actions">
                  <span className="text-caption text-text-tertiary">
                    {server.enabled ? server.health : t("settings.usenet.disabled")}
                  </span>
                  <Button
                    disabled={usenet.busy}
                    onClick={() => {
                      void usenet.test({
                        id: server.id,
                        name: server.name,
                        host: server.host,
                        port: server.port,
                        tls: server.tls,
                        username: server.username,
                        password: null,
                        connections: server.connections,
                        priority: server.priority,
                        enabled: server.enabled,
                      });
                    }}
                    size="sm"
                    variant="secondary"
                  >
                    {t("settings.usenet.test")}
                  </Button>
                  <Button
                    disabled={usenet.busy}
                    onClick={() => {
                      setForm({
                        id: server.id,
                        name: server.name,
                        host: server.host,
                        port: String(server.port),
                        tls: server.tls,
                        username: server.username,
                        password: "",
                        connections: String(server.connections),
                        priority: String(server.priority),
                        enabled: server.enabled,
                      });
                    }}
                    size="sm"
                    variant="secondary"
                  >
                    {t("settings.usenet.edit")}
                  </Button>
                  <Button
                    disabled={usenet.busy}
                    onClick={() => usenet.remove(server.id)}
                    size="sm"
                    variant="secondary"
                  >
                    {t("settings.usenet.remove")}
                  </Button>
                </div>
              </SettingsRow>
            ))
          )}

          <div className="usenet-editor">
            <div className="usenet-editor-heading">
              <div>
                <h3>
                  {form.id === null ? t("settings.usenet.add") : t("settings.usenet.editTitle")}
                </h3>
                <p>{t("settings.usenet.formDescription")}</p>
              </div>
              <Button disabled={usenet.busy} onClick={submit} size="sm">
                {t("settings.usenet.save")}
              </Button>
            </div>
            <div className="usenet-form-grid">
              <TextInput
                className="w-40"
                label={t("settings.usenet.name")}
                onChange={(event) => {
                  update({ name: event.target.value });
                }}
                value={form.name}
              />
              <TextInput
                className="w-56"
                label={t("settings.usenet.host")}
                onChange={(event) => {
                  update({ host: event.target.value });
                }}
                value={form.host}
              />
              <TextInput
                className="w-24"
                label={t("settings.usenet.port")}
                onChange={(event) => {
                  update({ port: event.target.value });
                }}
                value={form.port}
              />
              <TextInput
                className="w-40"
                label={t("settings.usenet.username")}
                onChange={(event) => {
                  update({ username: event.target.value });
                }}
                value={form.username}
              />
              <TextInput
                className="w-40"
                label={t("settings.usenet.password")}
                onChange={(event) => {
                  update({ password: event.target.value });
                }}
                type="password"
                value={form.password}
              />
              <TextInput
                className="w-24"
                label={t("settings.usenet.connections")}
                onChange={(event) => {
                  update({ connections: event.target.value });
                }}
                value={form.connections}
              />
            </div>
          </div>

          <SettingsRow label={t("settings.usenet.allowArchives")}>
            <Switch
              checked={usenet.status?.allowArchives ?? false}
              label={t("settings.usenet.allowArchives")}
              onChange={(next) => {
                void usenet.setArchives(next, usenet.status?.deleteArchivesAfterImport ?? true);
              }}
            />
          </SettingsRow>
          <SettingsRow label={t("settings.usenet.deleteArchives")}>
            <Switch
              checked={usenet.status?.deleteArchivesAfterImport ?? true}
              label={t("settings.usenet.deleteArchives")}
              onChange={(next) => {
                void usenet.setArchives(usenet.status?.allowArchives ?? false, next);
              }}
            />
          </SettingsRow>
          <SettingsRow label={t("settings.usenet.tools")}>
            <span className="text-caption text-muted">
              {t("settings.usenet.extract")}:{" "}
              {usenet.status?.extractAvailable
                ? t("settings.usenet.available")
                : t("settings.usenet.missing")}
              {" · "}
              {t("settings.usenet.repair")}:{" "}
              {usenet.status?.repairAvailable
                ? t("settings.usenet.available")
                : t("settings.usenet.missing")}
            </span>
          </SettingsRow>
          {usenet.error === null ? null : (
            <p className="text-caption text-danger" role="alert">
              {usenet.error}
            </p>
          )}
        </>
      )}
    </SettingsSection>
  );
}
