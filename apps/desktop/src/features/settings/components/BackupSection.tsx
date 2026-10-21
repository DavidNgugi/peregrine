import { Button, Dialog, SettingsRow, SettingsSection } from "@peregrine/ui";
import { useState, type ReactElement } from "react";
import { useTranslation } from "react-i18next";
import { pickFile } from "../../../lib/picker.js";
import { useBackup } from "../hooks/useBackup.js";

/** What the user is about to restore, once they have chosen it in the OS dialog. */
interface ChosenBackup {
  readonly path: string;
  readonly name: string;
}

/**
 * Backup and restore, as one settings group.
 *
 * The section owns the confirmation because a restore replaces the whole installation and the
 * only place that decision belongs is in front of the file the user chose. It does not own the
 * paths: the folder and file pickers are the shell's, and the core validates what they return.
 */
export function BackupSection(): ReactElement {
  const { t } = useTranslation();
  const backup = useBackup();
  const [chosen, setChosen] = useState<ChosenBackup | null>(null);

  const choose = async (): Promise<void> => {
    const picked = await pickFile({
      title: t("settings.backup.restore.pick"),
      filters: [
        {
          name: t("settings.backup.restore.filter"),
          extensions: ["sqlite", "db", "backup"],
        },
      ],
    });
    if (picked === null || picked.kind !== "files") {
      return;
    }
    const path = picked.paths[0];
    if (path === undefined) {
      return;
    }
    setChosen({ path, name: path.split(/[\\/]/).pop() ?? path });
  };

  return (
    <SettingsSection
      description={t("settings.backup.description")}
      title={t("settings.backup.title")}
    >
      <SettingsRow
        description={t("settings.backup.create.description")}
        label={t("settings.backup.create.label")}
      >
        <Button
          disabled={backup.busy}
          onClick={() => {
            void backup.create();
          }}
          size="sm"
          variant="secondary"
        >
          {t("settings.backup.create.action")}
        </Button>
      </SettingsRow>
      <SettingsRow
        description={t("settings.backup.restore.description")}
        label={t("settings.backup.restore.label")}
      >
        <Button
          disabled={backup.busy}
          onClick={() => {
            void choose();
          }}
          size="sm"
          variant="secondary"
        >
          {t("settings.backup.restore.action")}
        </Button>
      </SettingsRow>
      {backup.outcome?.kind === "created" ? (
        <p className="px-3 py-2 text-caption text-text-secondary" role="status">
          {t("settings.backup.created", { path: backup.outcome.backup.path })}
        </p>
      ) : null}
      {backup.outcome?.kind === "restoring" ? (
        <p className="px-3 py-2 text-caption text-text-secondary" role="status">
          {t("settings.backup.restoring")}
        </p>
      ) : null}
      {backup.error === null ? null : (
        <p className="px-3 py-2 text-caption text-state-danger" role="alert">
          {backup.error}
        </p>
      )}
      {chosen === null ? null : (
        <Dialog
          closeLabel={t("common.close")}
          description={t("settings.backup.confirm.body", { file: chosen.name })}
          onClose={() => {
            setChosen(null);
          }}
          title={t("settings.backup.confirm.title")}
        >
          <div className="flex justify-end gap-2">
            <Button
              onClick={() => {
                setChosen(null);
              }}
              size="sm"
              variant="ghost"
            >
              {t("settings.backup.confirm.cancel")}
            </Button>
            <Button
              onClick={() => {
                const source = chosen.path;
                setChosen(null);
                void backup.restore(source);
              }}
              size="sm"
              variant="danger"
            >
              {t("settings.backup.confirm.confirm")}
            </Button>
          </div>
        </Dialog>
      )}
    </SettingsSection>
  );
}
