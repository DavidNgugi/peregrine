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

import { Button, EmptyState, ErrorState, Progress, Screen, Section, Table } from "@peregrine/ui";
import type { TableColumn } from "@peregrine/ui";
import type { LibraryTitleDto, MatchRuleDto, UnmatchedFileDto } from "@peregrine/ipc-client";
import type { ReactElement } from "react";
import { useTranslation } from "react-i18next";
import { useEffect, useMemo } from "react";
import { messageOf } from "../../../lib/ipc-message.js";
import type { LibraryTitles } from "../hooks/useLibraryTitles.js";
import type { MatchRules } from "../hooks/useMatchRules.js";
import type { UnmatchedQueue } from "../hooks/useUnmatchedFiles.js";
import type { ScanControl } from "../hooks/useScan.js";

interface LibraryScreenProps {
  /** The titles as the shell read them. */
  readonly titles: LibraryTitles;
  /**
   * Opens one title's own screen.
   *
   * A callback rather than a link because the shell owns the route: the screen says *which* title, and
   * where that lives is the router's business (`app/routes.ts`).
   */
  readonly onOpenTitle: (title: LibraryTitleDto) => void;
  /**
   * The scan this screen can start, and the state of the one it is following.
   *
   * Passed in rather than created here, for the same reason the shortcut registry is: the shell
   * owns the subscription, so a scan started from Settings and a scan started from here are one
   * scan with one story.
   */
  readonly scan: ScanControl;
  /**
   * The files the scans could not place, and the action that answers one.
   *
   * Passed in like the rest: the shell owns the read, and a second one here could disagree with the
   * count the user has already been shown.
   */
  readonly unmatched: UnmatchedQueue;
  /**
   * The rules a user's answers became.
   *
   * Shown because a rule keeps acting: it places every later name that reads the way the one it was
   * made from did. An answer that cannot be seen cannot be withdrawn, and a wrong one would go on
   * placing files in silence.
   */
  readonly rules: MatchRules;
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
function columnsOf(
  t: Translate,
  open: (title: LibraryTitleDto) => void,
): readonly TableColumn<LibraryTitleDto>[] {
  return [
    {
      id: "name",
      header: t("library.column.title"),
      cell: (row) => (
        <span className="flex flex-col items-start">
          {/* A button rather than a link because the kit has no link yet (`PG-E21-03`'s list) and a
              title that opened nothing would be worse than one that looks like an action. */}
          <Button
            onClick={() => {
              open(row);
            }}
            size="sm"
            variant="ghost"
          >
            {row.name}
          </Button>
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

/**
 * The title a row's suggestion names, or `null` when it has none.
 *
 * Defensive on purpose. The contract says this field is a string or `null`, and a payload that omits
 * it — an older core, a stub, a hand-written fixture — arrives as `undefined`, at which point a check
 * for `null` takes the *suggestion* branch with nothing in it and renders a bare "Looks like". An
 * empty string is the same case: a label with a hole in it is worse than no label.
 */
function suggestionOf(row: UnmatchedFileDto): string | null {
  const named = row.suggestedTitleName;
  return typeof named === "string" && named !== "" ? named : null;
}

/** What the parse read, as the line under a parked file's name. */
function parsedLabel(row: UnmatchedFileDto, t: Translate): string {
  const parts = [
    row.parsedTitle,
    row.parsedYear === null ? null : `(${String(row.parsedYear)})`,
    row.parsedSeason === null ? null : `S${String(row.parsedSeason)}`,
    row.parsedEpisode === null ? null : `E${String(row.parsedEpisode)}`,
  ].filter((part): part is string => part !== null && part !== "");
  return parts.length === 0 ? t("library.queue.nothingRead") : parts.join(" ");
}

/**
 * The columns of the queue.
 *
 * The parse's values are shown *beside* the reason rather than instead of it: the question a person
 * is answering is which of the values the parse managed to read should be trusted, and hiding either
 * half would make that unanswerable.
 */
function queueColumnsOf(
  t: Translate,
  match: (file: UnmatchedFileDto) => void,
  busy: boolean,
): readonly TableColumn<UnmatchedFileDto>[] {
  return [
    {
      id: "name",
      header: t("library.queue.column.file"),
      cell: (row) => (
        <span className="flex flex-col">
          <span>{row.name}</span>
          <span className="text-caption text-text-tertiary">{parsedLabel(row, t)}</span>
        </span>
      ),
    },
    {
      id: "reason",
      header: t("library.queue.column.why"),
      cell: (row) => {
        const named = suggestionOf(row);
        return named === null ? (
          (row.reason ?? t("library.queue.noReason"))
        ) : (
          // The offer is what a person needs to answer without a picker: the library already holds a
          // title this name could mean, and saying so is more useful than repeating why it stopped.
          <span className="flex flex-col">
            <span>{t("library.queue.looksLike", { title: named })}</span>
            <span className="text-caption text-text-tertiary">
              {row.reason ?? t("library.queue.noReason")}
            </span>
          </span>
        );
      },
    },
    {
      id: "match",
      header: "",
      cell: (row) => {
        const named = suggestionOf(row);
        return (
          <Button
            disabled={busy || row.parsedTitle === null}
            onClick={() => {
              match(row);
            }}
            size="sm"
            variant="ghost"
          >
            {named === null
              ? t("library.queue.match")
              : t("library.queue.matchUnder", { title: named })}
          </Button>
        );
      },
    },
  ];
}

/**
 * The columns of the rules list.
 *
 * Two columns and a button: what a name reads, and the title it is placed under. The button says
 * "Forget" rather than "Remove" because what is being removed is the *rule*, not the file — the files
 * it placed stay, and a word that suggested otherwise would make a user think twice about using it.
 */
function ruleColumnsOf(
  t: Translate,
  forget: (id: number) => void,
  busy: boolean,
): readonly TableColumn<MatchRuleDto>[] {
  return [
    {
      id: "pattern",
      header: t("library.rules.column.name"),
      cell: (row) => (
        <span className="flex flex-col">
          <span>{row.pattern}</span>
          <span className="text-caption text-text-tertiary">{t("library.rules.readsAs")}</span>
        </span>
      ),
    },
    {
      id: "title",
      header: t("library.rules.column.placedUnder"),
      cell: (row) => row.titleName,
    },
    {
      id: "forget",
      header: "",
      cell: (row) => (
        <Button
          disabled={busy}
          onClick={() => {
            forget(row.id);
          }}
          size="sm"
          variant="ghost"
        >
          {t("library.rules.forget")}
        </Button>
      ),
    },
  ];
}

/** The library screen. */
export function LibraryScreen({
  titles,
  onOpenTitle,
  scan,
  unmatched,
  rules,
}: LibraryScreenProps): ReactElement {
  const { t } = useTranslation();
  const columns = useMemo(() => columnsOf(t, onOpenTitle), [t, onOpenTitle]);
  const { finished, refresh } = { finished: scan.finished, refresh: titles.refresh };

  // A scan is the only thing that changes this list, so watching one finish is the whole of the
  // subscription: the counter changes once per finished scan, and a boolean would refresh once and
  // then never again for the second.
  useEffect(() => {
    if (finished > 0) {
      void refresh();
    }
  }, [finished, refresh]);

  return (
    <Screen title={t("library.title")}>
      <Section description={t("library.description")} title={t("library.section")}>
        <div className="flex flex-col gap-2">
          <div className="flex items-center justify-between gap-4">
            {scan.running ? (
              <Progress label={t("library.scan.running")} value={scan.permille} />
            ) : (
              <span className="text-caption text-text-tertiary">
                {scan.phase === "succeeded"
                  ? t("library.scan.finished")
                  : scan.phase === "canceled"
                    ? t("library.scan.canceled")
                    : scan.phase === "failed"
                      ? t("library.scan.failed")
                      : t("library.scan.idle")}
              </span>
            )}
            <Button
              disabled={scan.starting || scan.running}
              onClick={() => {
                void scan.start();
              }}
              size="sm"
              variant="secondary"
            >
              {scan.running ? t("library.scan.working") : t("library.scan.action")}
            </Button>
          </div>
          {scan.error === null ? null : (
            <ErrorState body={messageOf(scan.error)} title={t("library.scan.refused")} />
          )}
        </div>

        {titles.loading ? null : titles.failed ? (
          <ErrorState body={t("library.unavailable.body")} title={t("library.unavailable.title")} />
        ) : titles.titles.length === 0 ? (
          <EmptyState body={t("library.empty.body")} size="page" title={t("library.empty.title")} />
        ) : (
          <Table columns={columns} rowKey={(row) => row.id} rows={titles.titles} />
        )}
      </Section>

      {unmatched.failed || unmatched.loading ? null : (
        <Section description={t("library.queue.description")} title={t("library.queue.title")}>
          {unmatched.error === null ? null : (
            <ErrorState body={unmatched.error} title={t("library.queue.refused")} />
          )}
          {unmatched.files.length === 0 ? (
            <EmptyState
              body={t("library.queue.empty.body")}
              size="inline"
              title={t("library.queue.empty.title")}
            />
          ) : (
            <Table
              columns={queueColumnsOf(t, (file) => void unmatched.match(file), unmatched.busy)}
              rowKey={(row) => row.id}
              rows={unmatched.files}
            />
          )}
        </Section>
      )}

      {rules.failed || rules.loading || rules.rules.length === 0 ? null : (
        <Section description={t("library.rules.description")} title={t("library.rules.title")}>
          {rules.error === null ? null : (
            <ErrorState body={rules.error} title={t("library.rules.refused")} />
          )}
          <Table
            columns={ruleColumnsOf(t, (id) => void rules.forget(id), rules.busy)}
            rowKey={(row) => row.id}
            rows={rules.rules}
          />
        </Section>
      )}
    </Screen>
  );
}
