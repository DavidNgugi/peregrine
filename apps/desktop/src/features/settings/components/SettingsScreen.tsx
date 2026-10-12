import {
  EmptyState,
  ErrorState,
  Screen,
  SettingsRow,
  SettingsSection,
  ShortcutKeys,
  Switch,
} from "@peregrine/ui";
import type { ReactElement } from "react";
import { useTranslation } from "react-i18next";
import { useAutoStart } from "../hooks/useAutoStart.js";
import type { LibraryRoots } from "../hooks/useLibraryRoots.js";
import type { ShortcutRegistry } from "../hooks/useShortcuts.js";
import { LibrariesSection } from "./LibrariesSection.js";

interface SettingsScreenProps {
  /**
   * The shortcut registry as the shell read it.
   *
   * Passed in rather than read here so the shell and this list read one answer: a second
   * fetch could disagree with the keys the shell is already dispatching on.
   */
  readonly shortcuts: ShortcutRegistry;

  /**
   * The library folders, as the shell read them.
   *
   * Passed in for the same reason the shortcut registry is: the shell owns the read, and a second
   * one here could disagree with the list the rest of the app is working from.
   */
  readonly libraries: LibraryRoots;
}

/**
 * Settings, as far as this build has anything to put in it.
 *
 * Every element is the kit's — `SettingsSection` owns the group and its heading,
 * `SettingsRow` owns a row, `Switch` owns the toggle, `ShortcutKeys` owns the accelerator —
 * so this file holds copy and state and no visual classes at all (PG-E7-03).
 *
 * Three states of the shortcut list are distinguished rather than collapsed into "empty":
 * still reading, failed, and genuinely empty. They say different things to the user, and the
 * middle one is the difference between "you have no shortcuts" and "Peregrine could not
 * ask". The settings registry (PG-E20-01) takes this screen over; nothing here is a setting
 * of our own.
 */
export function SettingsScreen({ shortcuts, libraries }: SettingsScreenProps): ReactElement {
  const { t } = useTranslation();
  const autoStart = useAutoStart();

  return (
    <Screen title={t("settings.title")}>
      <SettingsSection
        description={t("settings.shortcuts.description")}
        title={t("settings.shortcuts.title")}
      >
        {shortcuts.loading ? null : shortcuts.failed ? (
          <ErrorState
            body={t("settings.shortcuts.unavailable.body")}
            title={t("settings.shortcuts.unavailable.title")}
          />
        ) : shortcuts.bindings.length === 0 ? (
          <EmptyState title={t("settings.shortcuts.empty")} />
        ) : (
          shortcuts.bindings.map((binding) => (
            <SettingsRow key={binding.id} label={t(binding.labelKey)}>
              <ShortcutKeys display={binding.display} />
            </SettingsRow>
          ))
        )}
      </SettingsSection>

      <SettingsSection
        description={t("settings.autostart.description")}
        title={t("settings.autostart.title")}
      >
        <Switch
          busy={autoStart.busy}
          checked={autoStart.enabled}
          label={t("settings.autostart.label")}
          onChange={(next) => {
            autoStart.toggle(next);
          }}
        />
        {autoStart.failed ? <ErrorState title={t("settings.autostart.failed")} /> : null}
      </SettingsSection>

      <LibrariesSection libraries={libraries} />
    </Screen>
  );
}
