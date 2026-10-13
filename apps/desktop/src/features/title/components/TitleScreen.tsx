/**
 * One title's own screen: what it is, what the library holds of it, its poster, and its episodes.
 *
 * The screen is named by the *route* as well as by the read, which is why the kind travels in the
 * path: a film's page can say "Film" before the core has answered, and a page that waited for its
 * read to say anything at all would be the full-page spinner `PG-E7-02` forbids.
 *
 * The poster is what the cache already holds. Filling that cache is the refresh job the hook asks
 * for when the core says one is due, and this screen neither waits for it nor draws anything from it:
 * a picture that arrives late is a picture that arrives late (`ARCHITECTURE §7` rule 2).
 *
 * **What is still owed here** is the rest of a title page: the hero expansion and the backdrop
 * cross-fade, cast, provider availability, and the actions that acquire or play — `PG-E7-11`, `PG-E5`,
 * `PG-E14-11`, and `PG-E15`.
 */

import { EmptyState, ErrorState, Screen, Section, Table } from "@peregrine/ui";
import type { TableColumn } from "@peregrine/ui";
import type { EpisodeDto, LibraryTitleDto } from "@peregrine/ipc-client";
import type { ReactElement } from "react";
import { useTranslation } from "react-i18next";
import { useMemo } from "react";
import { TitleArtwork } from "./TitleArtwork.js";
import type { TitleDetail } from "../hooks/useTitle.js";
import type { TitleKind } from "../../../app/routes.js";

interface TitleScreenProps {
  /** Which of the two the route says this is, which is what the page can say immediately. */
  readonly kind: TitleKind;
  /** The title and its episodes as the shell read them. */
  readonly detail: TitleDetail;
}

/** What `useTranslation` hands a component. */
type Translate = ReturnType<typeof useTranslation>["t"];

/** Bytes as a row shows them: whole units, the same way the library list does. */
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

/** The episode columns: where it sits, what it holds, and how big it is. */
function episodeColumns(t: Translate): readonly TableColumn<EpisodeDto>[] {
  return [
    {
      id: "number",
      header: t("title.column.episode"),
      cell: (row) => `${String(row.seasonNumber)}×${String(row.episodeNumber).padStart(2, "0")}`,
    },
    {
      id: "name",
      header: t("title.column.name"),
      cell: (row) =>
        row.name ?? <span className="text-text-tertiary">{t("title.unnamedEpisode")}</span>,
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

/** The title screen. */
export function TitleScreen({ kind, detail }: TitleScreenProps): ReactElement {
  const { t } = useTranslation();
  const columns = useMemo(() => episodeColumns(t), [t]);
  const title: LibraryTitleDto | null = detail.title;

  // The route's kind is the fallback, so the page names itself before the read answers and keeps the
  // same heading after — a heading that changed once the core replied would be a visible flicker.
  const kindLabel =
    title === null
      ? kind === "movie"
        ? t("library.kind.films")
        : t("library.kind.series")
      : title.kind === "movies"
        ? t("library.kind.films")
        : t("library.kind.series");

  return (
    <Screen title={title?.name ?? kindLabel}>
      <Section description={kindLabel} title={t("title.section")}>
        {detail.loading ? null : detail.failed ? (
          <ErrorState body={t("title.unavailable.body")} title={t("title.unavailable.title")} />
        ) : detail.missing || title === null ? (
          <EmptyState body={t("title.missing.body")} size="page" title={t("title.missing.title")} />
        ) : (
          // The poster beside the counts rather than above them: the two are one answer about one
          // title, and a reader who has the picture should not have to scroll past it to learn what
          // the library holds.
          <div className="flex flex-wrap items-start gap-6">
            <TitleArtwork name={title.name} poster={detail.poster} />
            <div className="min-w-[16rem] flex-1">
              <Table
                columns={[
                  {
                    id: "items",
                    header: t("title.column.items"),
                    numeric: true,
                    cell: () => String(title.items),
                  },
                  {
                    id: "files",
                    header: t("library.column.files"),
                    numeric: true,
                    cell: () => String(title.files),
                  },
                  {
                    id: "size",
                    header: t("library.column.size"),
                    numeric: true,
                    cell: () => sizeLabel(title.bytes, t),
                  },
                  {
                    id: "missing",
                    header: t("library.column.missing"),
                    numeric: true,
                    cell: () => (title.missingFiles === 0 ? "—" : String(title.missingFiles)),
                  },
                ]}
                rowKey={() => "totals"}
                rows={[title]}
              />
            </div>
          </div>
        )}
      </Section>

      {title === null || title.kind === "movies" || detail.episodes.length === 0 ? null : (
        <Section description={t("title.episodes.description")} title={t("title.episodes.title")}>
          <Table
            columns={columns}
            rowKey={(row) => `${row.seasonNumber}-${row.episodeNumber}`}
            rows={detail.episodes}
          />
        </Section>
      )}
    </Screen>
  );
}
