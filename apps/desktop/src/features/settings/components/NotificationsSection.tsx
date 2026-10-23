import { ErrorState, SettingsSection, Switch } from "@peregrine/ui";
import type { ReactElement } from "react";
import { useTranslation } from "react-i18next";

import type { NotificationConsentControl } from "../hooks/useNotificationConsent.js";

interface NotificationsSectionProps {
  readonly control: NotificationConsentControl;
}

/**
 * The notification consent switch.
 *
 * **The note is the ask.** R31 requires the user to be asked before we notify them, and the one
 * thing the toggle must say plainly is that turning it on raises a system prompt — a control that
 * silently called `requestPermission` would be asking the OS for something the person did not
 * expect. The in-app centre is named beside it because it is the record whatever the system says.
 */
export function NotificationsSection({ control }: NotificationsSectionProps): ReactElement {
  const { t } = useTranslation();
  const disabled = control.preferences === null || control.loading || control.failed;

  return (
    <SettingsSection
      description={t("settings.notifications.description")}
      title={t("settings.notifications.title")}
    >
      <Switch
        busy={control.busy}
        checked={control.preferences?.consent === "granted"}
        disabled={disabled}
        label={t("settings.notifications.label")}
        onChange={(next) => {
          control.toggle(next);
        }}
      />
      <p className="px-3 py-2 text-caption text-text-tertiary">
        {t("settings.notifications.note")}
      </p>
      {control.denied ? (
        <p className="px-3 pb-2 text-caption text-state-danger" role="status">
          {t("settings.notifications.denied")}
        </p>
      ) : null}
      {control.failed ? <ErrorState title={t("settings.notifications.failed")} /> : null}
      {control.saveFailed ? (
        <ErrorState title={t("settings.notifications.saveFailed")} />
      ) : null}
    </SettingsSection>
  );
}
