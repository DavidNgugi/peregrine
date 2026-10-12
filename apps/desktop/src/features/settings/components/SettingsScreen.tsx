import type { ShortcutBinding } from "@peregrine/ipc-client";
import type { ReactElement } from "react";
import { useTranslation } from "react-i18next";
import { useAutoStart } from "../hooks/useAutoStart.js";

interface SettingsScreenProps {
  /**
   * The shortcut registry as the core sent it.
   *
   * Passed in rather than read here so the shell and this list read one answer: a second
   * fetch could disagree with the keys the shell is already dispatching on.
   */
  readonly bindings: readonly ShortcutBinding[];
}

/**
 * Settings, as far as Phase 1 has anything to put in it.
 *
 * Two sections, both of which are a projection of state that lives elsewhere: the
 * shortcut list is generated from the core's registry (PG-E1-08) and the login-item
 * toggle reads and writes what the OS reports (PG-E1-13). The settings registry
 * (PG-E20-01) takes this screen over; nothing here is a setting of our own.
 */
export function SettingsScreen({ bindings }: SettingsScreenProps): ReactElement {
  const { t } = useTranslation();
  const autoStart = useAutoStart();

  return (
    <section
      aria-labelledby="settings-title"
      className="flex h-full flex-col gap-6 overflow-y-auto px-6 py-4"
    >
      <h1 className="text-title-1 text-text-primary" id="settings-title">
        {t("settings.title")}
      </h1>

      <section aria-labelledby="settings-shortcuts-title" className="flex flex-col gap-3">
        <div className="flex flex-col gap-1">
          <h2 className="text-title-2 text-text-primary" id="settings-shortcuts-title">
            {t("settings.shortcuts.title")}
          </h2>
          <p className="text-caption text-text-tertiary">{t("settings.shortcuts.description")}</p>
        </div>
        <ul className="flex max-w-lg flex-col divide-y divide-border-subtle rounded-md border border-border-subtle bg-surface-2">
          {bindings.map((binding) => (
            <li className="flex items-center justify-between gap-4 px-3 py-2" key={binding.id}>
              <span className="text-body text-text-primary">{t(binding.labelKey)}</span>
              <kbd className="rounded-sm border border-border-subtle bg-surface-1 px-2 py-0.5 font-mono text-caption text-text-secondary">
                {binding.display}
              </kbd>
            </li>
          ))}
        </ul>
      </section>

      <section aria-labelledby="settings-autostart-title" className="flex flex-col gap-3">
        <div className="flex flex-col gap-1">
          <h2 className="text-title-2 text-text-primary" id="settings-autostart-title">
            {t("settings.autostart.title")}
          </h2>
          <p className="text-caption text-text-tertiary">{t("settings.autostart.description")}</p>
        </div>
        <label className="flex max-w-lg items-center gap-3 text-body text-text-primary">
          <input
            checked={autoStart.enabled}
            className="size-4 accent-accent"
            disabled={autoStart.busy}
            onChange={(event) => {
              autoStart.toggle(event.target.checked);
            }}
            type="checkbox"
          />
          {t("settings.autostart.label")}
        </label>
        {autoStart.failed ? (
          <p className="text-caption text-state-danger" role="alert">
            {t("settings.autostart.failed")}
          </p>
        ) : null}
      </section>
    </section>
  );
}
