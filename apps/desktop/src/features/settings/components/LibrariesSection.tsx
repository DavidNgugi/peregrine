/**
 * The folders the core is allowed to scan, and the two things a user does about them.
 *
 * Three states are shown as three different things rather than collapsed into an empty list: the
 * read is still in flight, the read failed, and there are genuinely no libraries. The middle one is
 * the difference between "you have not added anything" and "Peregrine could not ask", and a screen
 * that showed the second as the first would be telling the user something it does not know.
 *
 * A root is listed by its path, because that is what identifies it to the person who chose it —
 * there is no title or artwork for a folder. What the row says beside the path is what the core
 * holds about it: what it is expected to hold, and when it was last scanned.
 */

import {
  Button,
  Dialog,
  EmptyState,
  ErrorState,
  SettingsRow,
  SettingsSection,
} from "@peregrine/ui";
import { useTranslation } from "react-i18next";
import type { LibraryRootDto, RootKind } from "@peregrine/ipc-client";
import type { ReactElement } from "react";
import { useState } from "react";
import type { LibraryRoots } from "../hooks/useLibraryRoots.js";

interface LibrariesSectionProps {
  /** The roots as the shell read them, with the operations this list offers. */
  readonly libraries: LibraryRoots;
}

/** The kind a folder added from this screen is filed as. */
const ADDED_KIND: RootKind = "mixed";

/**
 * What a folder is expected to hold, in the user's words.
 *
 * The kind is the user's answer rather than a guess (PG-E14-01), and this screen does not ask for
 * it yet: a folder is added as `mixed`, which is the honest answer for a folder nobody has
 * classified and the one that scans it for both films and series. Choosing the kind and the rescan
 * mode arrives with the settings registry (PG-E20-01), which owns every control on this screen.
 */
function kindLabel(kind: RootKind, t: (key: string) => string): string {
  return t(`settings.libraries.kind.${kind}`);
}

/**
 * What is known about one root, as the line under its path.
 *
 * The instant is not rendered. Nothing in this build formats a date for a user yet — that belongs
 * with the i18n work that owns number and date formatting, which is where a locale-aware
 * "scanned 2 hours ago" will come from — and a hand-rolled one here would be the first of several.
 * Whether a root has ever been scanned is the fact the row can state truthfully today.
 */
function describe(root: LibraryRootDto, t: (key: string) => string): string {
  return t(
    root.lastScanAt === null ? "settings.libraries.neverScanned" : "settings.libraries.scanned",
  ).replace("{{kind}}", kindLabel(root.kind, t));
}

/** The library folders section of the settings screen. */
export function LibrariesSection({ libraries }: LibrariesSectionProps): ReactElement {
  const { t } = useTranslation();
  const busy = libraries.busy;
  const [removing, setRemoving] = useState<LibraryRootDto | null>(null);

  return (
    <SettingsSection
      description={t("settings.libraries.description")}
      title={t("settings.libraries.title")}
    >
      {libraries.failed ? (
        <ErrorState
          body={t("settings.libraries.unavailable.body")}
          title={t("settings.libraries.unavailable.title")}
        />
      ) : null}

      {!libraries.failed && libraries.error !== null ? (
        <ErrorState body={libraries.error} title={t("settings.libraries.refused.title")} />
      ) : null}

      {!libraries.failed && !libraries.loading && libraries.roots.length === 0 ? (
        <EmptyState
          body={t("settings.libraries.empty.body")}
          size="inline"
          title={t("settings.libraries.empty.title")}
        />
      ) : null}

      {libraries.roots.map((root) => (
        <SettingsRow description={describe(root, t)} key={root.id} label={root.path}>
          <Button
            disabled={busy}
            onClick={() => {
              setRemoving(root);
            }}
            size="sm"
            variant="danger"
          >
            {t("settings.libraries.remove")}
          </Button>
        </SettingsRow>
      ))}

      {removing === null ? null : (
        <Dialog
          closeLabel={t("settings.libraries.removeConfirm.cancel")}
          description={t("settings.libraries.removeConfirm.body", { path: removing.path })}
          onClose={() => {
            setRemoving(null);
          }}
          title={t("settings.libraries.removeConfirm.title")}
        >
          <div className="flex justify-end gap-2">
            <Button
              onClick={() => {
                setRemoving(null);
              }}
              size="sm"
              variant="ghost"
            >
              {t("settings.libraries.removeConfirm.cancel")}
            </Button>
            <Button
              onClick={() => {
                const target = removing;
                setRemoving(null);
                if (target !== null) {
                  void libraries.remove(target.id);
                }
              }}
              size="sm"
              variant="danger"
            >
              {t("settings.libraries.removeConfirm.confirm")}
            </Button>
          </div>
        </Dialog>
      )}

      <SettingsRow
        description={t("settings.libraries.add.description")}
        label={t("settings.libraries.add.label")}
      >
        <Button
          disabled={busy || libraries.failed}
          onClick={() => {
            void libraries.add(ADDED_KIND);
          }}
          size="sm"
          variant="primary"
        >
          {t("settings.libraries.add.action")}
        </Button>
      </SettingsRow>

      <SettingsRow
        description={t("settings.libraries.scan.description")}
        label={t("settings.libraries.scan.label")}
      >
        <Button
          disabled={busy || libraries.failed || libraries.roots.length === 0}
          onClick={() => {
            void libraries.scan();
          }}
          size="sm"
          variant="secondary"
        >
          {t("settings.libraries.scan.action")}
        </Button>
      </SettingsRow>
    </SettingsSection>
  );
}
