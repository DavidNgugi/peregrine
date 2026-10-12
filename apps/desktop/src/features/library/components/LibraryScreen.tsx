/**
 * The library: the titles the scans have put here, and what the core holds of each.
 *
 * This is the app's own content rather than the catalogue's — what is on this machine, found by
 * walking the folders the user configured — which is why it is a screen of its own beside Discover
 * rather than a filter on it. Discover is what could be acquired; this is what is here.
 *
 * Every element is the kit's, so this file holds structure and copy and no visual classes at all:
 * `Screen` owns the page, `Section` the heading, `Table` the columns, and the two state blocks the
 * empty and failed cases.
 *
 * A **stub** is a title the library inferred from a filename and no provider has described yet
 * (PG-E5-06). The table says so on the row rather than hiding it, because it is the honest answer to
 * "why is there no poster": the metadata road is a later slice, and a screen that drew these rows
 * like any other would promise artwork that is not coming yet.
 */

import { EmptyState, ErrorState, Screen, Section, Table } from "@peregrine/ui";
import type { TableColumn } from "@peregrine/ui";
import type { LibraryTitleDto } from "@peregrine/ipc-client";
import type { ReactElement } from "react";
import { useTranslation } from "react-i18next";
import { useMemo } from "react";
import type { LibraryTitles } from "../hooks/useLibraryTitles.js";

interface LibraryScreenProps {
  /** The titles as the shell read them. */
  readonly titles: LibraryTitles;
}

/** What `useTranslation` hands a component, so the column builder can take it by name. */
type Translate = ReturnType<typeof useTranslation>["t"];

/** Bytes as the row shows them: whole units, because a library is not a download progress bar. */
function sizeLabel(bytes: number, t: Translate): string {
  if (bytes === 0) {
    return t("library.size.none");
  }
  const units = ["B", "KB", "MB", "GB", "TB"];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  const rounded = value < 10 ? Math.round(value * 10) / 10 : Math.round(value);
  return t("library.size.value", { value: rounded, unit: units[unit] ?? "B" });
}

/**
 * The columns, built once per language rather than per row.
 *
 * The library's own read, so a row is a title with what the core holds of it: how many copies, how
 * big they are, and how many of them the last scan could not find. `missing` is a count rather than
 * a flag because a series can have one episode on a drive that is not plugged in and twenty that
 * are.
 */
function columnsOf(t: Translate): readonly TableColumn<LibraryTitleDto>[] {
  return [
    {
      id: "name",
      header: t("library.column.title"),
      cell: (row) => (
        <span className="flex flex-col">
          <span>{row.name}</span>
          {row.stub ? (
            <span className="text-caption text-text-tertiary">{t("library.stub")}</span>
          ) : null}
        </span>
      ),
    },
    {
      id: "kind",
      header: t("library.column.kind"),
      cell: (row) => (row.kind === "movies" ? t("library.kind.films") : t("library.kind.series")),
    },
    {
      id: "files",
      header: t("library.column.files"),
      numeric: true,
      cell: (row) => String(row.files),
    },
    {
      id: "size",
      header: t("library.column.size"),
      numeric: true,
      cell: (row) => sizeLabel(row.bytes, t),
    },
    {
      id: "missing",
      header: t("library.column.missing"),
      numeric: true,
      cell: (row) => (row.missingFiles === 0 ? "—" : String(row.missingFiles)),
    },
  ];
}

/** The library screen. */
export function LibraryScreen({ titles }: LibraryScreenProps): ReactElement {
  const { t } = useTranslation();
  const columns = useMemo(() => columnsOf(t), [t]);

  return (
    <Screen title={t("library.title")}>
      <Section description={t("library.description")} title={t("library.section")}>
        {titles.loading ? null : titles.failed ? (
          <ErrorState body={t("library.unavailable.body")} title={t("library.unavailable.title")} />
        ) : titles.titles.length === 0 ? (
          <EmptyState body={t("library.empty.body")} size="page" title={t("library.empty.title")} />
        ) : (
          <Table columns={columns} rowKey={(row) => row.id} rows={titles.titles} />
        )}
      </Section>
    </Screen>
  );
}
