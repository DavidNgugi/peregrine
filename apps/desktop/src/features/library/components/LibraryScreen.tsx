/**
 * The library: the titles the scans have put here, and what the core holds of each.
 *
 * This is the app's own content rather than the catalogue's — what is on this machine, found by
 * walking the folders the user configured — which is why it is a screen of its own beside Discover
 * rather than a filter on it. Discover is what could be acquired; this is what is here.
 *
 * **One read, two drawings.** The core answers each title with its counts *and* the cached poster the
 * grid needs, and this screen draws that answer as a table or as a wall of tiles — the choice is
 * `useLibraryView`'s, owned by the shell and handed in as a prop, and it never reaches the core,
 * because both drawings show the same titles in the same order. Every element is the kit's, so this
 * file holds structure and copy and no visual classes at all: `Screen` owns the page, `Section` the
 * heading, `Table` the columns, `Grid` the windowed tiles, and the two state blocks the empty and
 * failed cases.
 *
 * A **stub** is a title the library inferred from a filename and no provider has described yet
 * (PG-E5-06). Both drawings say so rather than hiding it, because it is the honest answer to "why is
 * there no poster": the metadata road is a later slice, and a row — or a tile — drawn like any other
 * would promise artwork that is not coming yet.
 */

import {
  Button,
  Dialog,
  EmptyState,
  ErrorState,
  Grid,
  Link,
  Progress,
  Screen,
  Section,
  Table,
  TextInput,
  TitleCard,
} from "@peregrine/ui";
import type { TableColumn } from "@peregrine/ui";
import type {
  LibraryTitleDto,
  MatchRuleDto,
  TitleMatchDto,
  UnmatchedFileDto,
} from "@peregrine/ipc-client";
import type { ReactElement } from "react";
import { useTranslation } from "react-i18next";
import { useEffect, useMemo, useState } from "react";
import { messageOf } from "../../../lib/ipc-message.js";
import { imageUrl } from "../../../lib/media.js";
import { useViewState } from "../../../lib/view-state.js";
import { usePosterBackfill } from "../../player/index.js";
import { routePath, type Route } from "../../../app/routes.js";
import { downloadCardProps, type DownloadMarks } from "../../../lib/download-status.js";
import { useDownloadProgress } from "../../../lib/download-progress.js";
import { TitlePicker } from "./TitlePicker.js";
import type { LibraryTitles } from "../hooks/useLibraryTitles.js";
import type { MatchRules } from "../hooks/useMatchRules.js";
import type { UnmatchedQueue } from "../hooks/useUnmatchedFiles.js";
import type { ScanControl } from "../hooks/useScan.js";
import type { LibraryView } from "../hooks/useLibraryView.js";

/**
 * A `lg` tile's width from §4's ladder, and the row height that goes with it.
 *
 * A row is the poster's height plus the gap, and the poster's height is the width over the 2:3 aspect
 * every card keeps. Fixed numbers rather than a measurement, because that is what makes the grid's
 * window arithmetic possible (`packages/ui`'s `Grid`); they are Browse's, so one title seen on both
 * screens is the same size.
 */
const TILE_WIDTH = 176;
const TILE_POSTER_HEIGHT = TILE_WIDTH * 1.5;
const TILE_ROW_HEIGHT = TILE_POSTER_HEIGHT + 12;

/** Nothing downloading, for a caller that draws the screen without a queue. */
const NO_DOWNLOADS: DownloadMarks = new Map();

interface LibraryScreenProps {
  /** The titles as the shell read them. */
  readonly titles: LibraryTitles;
  /**
   * Moves to a route.
   *
   * The shell owns where the application *is*; the screen says where a link points, and the route
   * table (`app/routes.ts`) is the one place a path is spelled. Passing a route rather than an id
   * keeps the mapping out of the shell, which had it only because there was no link to carry it.
   */
  readonly onNavigate: (route: Route) => void;
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
  /**
   * Which of the two drawings is on screen.
   *
   * Passed in for the reason the read is: the choice outlives this screen, because `AppShell` renders
   * one screen at a time and unmounts this one whenever the route moves — so a `useState` here would
   * reset the drawing on every visit. `useLibraryView` is that state, and the shell is what owns it.
   */
  readonly view: LibraryView;
  /** Changes which drawing is on screen. */
  readonly onView: (view: LibraryView) => void;
  /** A download status per title, so a tile wears the mark of a transfer in flight. */
  readonly downloads?: DownloadMarks | undefined;
  /** Titles on the account's watchlist, so their tiles carry the bookmark. */
  readonly watchlisted?: ReadonlySet<number> | undefined;
  /** The shell's back control, present only when there is somewhere behind this screen. */
  readonly onBack?: (() => void) | undefined;
  readonly backLabel?: string | undefined;
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
 * A tile's one line: what it is, what it weighs, and whether anything is missing.
 *
 * **Every piece of it is a count the core answered, and the omissions are deliberate.** The kind is
 * the title's own; the size is the files behind it; the missing count is the scan's. What is *not*
 * here is an availability chip, and that is `AGENTS R7` rather than an oversight: which of the nine
 * availability states a title is in is `peregrine_domain::availability`'s rule, and the library read
 * does not carry it — computing one from two counts in TypeScript would be exactly the derivation the
 * rule forbids. The list states both counts in their own columns, so the grid hides nothing the table
 * shows; it draws fewer of them, which is what a cover is for.
 */
function tileMeta(row: LibraryTitleDto, t: Translate): string {
  const parts = [
    row.kind === "movies" ? t("library.kind.films") : t("library.kind.series"),
    sizeLabel(row.bytes, t),
    row.missingFiles === 0 ? null : t("library.tile.missing", { count: row.missingFiles }),
    row.stub ? t("library.stub") : null,
  ].filter((part): part is string => part !== null);
  return parts.join(" · ");
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
  open: (route: Route) => void,
): readonly TableColumn<LibraryTitleDto>[] {
  return [
    {
      id: "name",
      header: t("library.column.title"),
      cell: (row) => (
        <span className="flex flex-col items-start">
          {/* A link, because that is what it is: it says where it goes, a screen reader announces it
              as a link, and a keyboard user opens it with `Enter` rather than activating a button. */}
          <Link
            href={routePath(routeFor(row))}
            onNavigate={() => {
              open(routeFor(row));
            }}
          >
            {row.name}
          </Link>
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

/** The route a title's own screen lives at, which is also the path its link carries. */
function routeFor(title: LibraryTitleDto): Route {
  return {
    kind: "title",
    titleKind: title.kind === "movies" ? "movie" : "show",
    id: title.id,
  };
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
  choose: (file: UnmatchedFileDto) => void,
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
      // Named rather than blank: an unnamed column is one a screen-reader table navigation cannot
      // announce, and the buttons in it all act on the parked file.
      header: t("library.column.actions"),
      cell: (row) => {
        const named = suggestionOf(row);
        return (
          <span className="flex items-center justify-end gap-1">
            <Button
              disabled={busy || row.parsedTitle === null}
              onClick={() => {
                match(row);
              }}
              size="sm"
              variant="secondary"
            >
              {named === null
                ? t("library.queue.match")
                : t("library.queue.matchUnder", { title: named })}
            </Button>
            {/* Offered on every row, including the ones with an offer: the parse and the scan can
                both be wrong, and a file nobody can place is a file the library never has. */}
            <Button
              disabled={busy}
              onClick={() => {
                choose(row);
              }}
              size="sm"
              variant="secondary"
            >
              {t("library.queue.choose")}
            </Button>
          </span>
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
  ask: (rule: MatchRuleDto) => void,
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
      header: t("library.column.actions"),
      cell: (row) => (
        <Button
          disabled={busy}
          onClick={() => {
            ask(row);
          }}
          size="sm"
          variant="danger"
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
  onNavigate,
  scan,
  unmatched,
  rules,
  view,
  onView,
  downloads,
  watchlisted,
  onBack,
  backLabel,
}: LibraryScreenProps): ReactElement {
  const { t } = useTranslation();
  const marks = downloads ?? NO_DOWNLOADS;
  const progress = useDownloadProgress();
  const columns = useMemo(() => columnsOf(t, onNavigate), [t, onNavigate]);
  const { finished, refresh } = { finished: scan.finished, refresh: titles.refresh };
  // **The library is where the wall of placeholders is, so it is where the frames are asked for.** A
  // title with no poster gets one captured from its own file, one at a time, and the grid redraws when a
  // batch finishes rather than per poster — a read per captured frame would be a hundred reads.
  const wanting = useMemo(
    () => titles.titles.filter((row) => row.needsPoster).map((row) => row.id),
    [titles.titles],
  );
  const backfill = usePosterBackfill(wanting);
  // A rule that keeps acting is worth one question before it is withdrawn: the files it placed stay,
  // but the answer it remembers stops applying, and a mis-click that changed how the library behaves
  // from then on would be a change nobody asked for (AGENTS R22).
  const [forgetting, setForgetting] = useState<MatchRuleDto | null>(null);
  // Which parked file the picker is open for, if any.
  const [choosing, setChoosing] = useState<UnmatchedFileDto | null>(null);
  // The search and the kind narrow a list the core already answered; neither changes what the
  // library holds. They outlive this screen so Back returns to the same filtered list.
  const [query, setQuery] = useViewState("library.query", "");
  const [kind, setKind] = useViewState<"all" | "movies" | "shows">("library.kind", "all");
  const shown = useMemo(() => {
    const needle = query.trim().toLowerCase();
    return titles.titles.filter(
      (row) =>
        (kind === "all" || row.kind === kind) &&
        (needle.length === 0 || row.name.toLowerCase().includes(needle)),
    );
  }, [titles.titles, query, kind]);

  // A scan is the only thing that changes this list, so watching one finish is the whole of the
  // subscription: the counter changes once per finished scan, and a boolean would refresh once and
  // then never again for the second.
  useEffect(() => {
    if (finished > 0) {
      void refresh();
    }
  }, [finished, refresh]);

  return (
    <Screen backLabel={backLabel} onBack={onBack} title={t("library.title")}>
      <Section description={t("library.description")} title={t("library.section")}>
        <div className="flex flex-col gap-2">
          {titles.storage === null || titles.titles.length === 0 ? null : (
            <p className="text-body text-text-secondary">
              {t("library.storage.summary", {
                count: titles.titles.length,
                size:
                  titles.storage.bytes === 0
                    ? t("library.storage.none")
                    : sizeLabel(titles.storage.bytes, t),
              })}
            </p>
          )}
          <div className="flex flex-wrap items-center justify-between gap-3">
            {scan.running ? (
              <Progress label={t("library.scan.running")} value={scan.permille} />
            ) : null}
            {/* The status region is mounted for the whole scan rather than appearing with its
                sentence: a region that arrives with its text is announced by some readers and not
                others, and this is the only signal for a reader who cannot see the bar. Hidden while
                the bar is on screen because the bar's own label says the same thing. */}
            <span
              className={scan.running ? "sr-only" : "text-caption text-text-tertiary"}
              role="status"
            >
              {scan.running
                ? t("library.scan.running")
                : scan.phase === "succeeded"
                  ? t("library.scan.finished")
                  : scan.phase === "canceled"
                    ? t("library.scan.canceled")
                    : scan.phase === "failed"
                      ? t("library.scan.failed")
                      : t("library.scan.idle")}
            </span>
            <div className="flex items-center gap-3">
              {/* Poster progress, said in words because a capture takes seconds and a screen that showed
                  nothing would look stuck. It disappears when the work does. */}
              {backfill.captured === 0 && !backfill.running ? null : (
                <span className="text-caption text-text-tertiary" role="status">
                  {backfill.running
                    ? t("library.posters.working", { count: backfill.captured })
                    : t("library.posters.done", { count: backfill.captured })}
                </span>
              )}
              {/* The two drawings of one read, and it is deliberately *not* part of the rows below: it
                  changes nothing about what is read, only how it is drawn, so it belongs with the
                  screen's own controls rather than with the answer. Drawn only once there is something
                  to draw — a toggle over an empty library would be a control for a choice nobody can
                  see. */}
              {titles.loading || titles.failed || titles.titles.length === 0 ? null : (
                <div
                  aria-label={t("library.view.label")}
                  className="flex items-center gap-1 rounded-full border border-border-subtle bg-surface-0/70 p-1"
                  data-library-view=""
                  role="group"
                >
                  {(["list", "grid"] as const).map((option) => (
                    <Button
                      aria-pressed={view === option}
                      className="rounded-full"
                      key={option}
                      onClick={() => {
                        onView(option);
                      }}
                      size="sm"
                      variant={view === option ? "secondary" : "ghost"}
                    >
                      {t(option === "list" ? "library.view.list" : "library.view.grid")}
                    </Button>
                  ))}
                </div>
              )}
              <Button
                disabled={scan.starting || scan.running}
                onClick={() => scan.start()}
                size="sm"
                variant="secondary"
              >
                {scan.running ? t("library.scan.working") : t("library.scan.action")}
              </Button>
            </div>
          </div>
          {scan.error === null ? null : (
            <ErrorState body={messageOf(scan.error)} title={t("library.scan.refused")} />
          )}
        </div>

        <div className="flex flex-wrap items-end gap-3">
          <div className="min-w-[16rem] flex-1">
            <TextInput
              label={t("library.search.label")}
              onChange={(event) => {
                setQuery(event.target.value);
              }}
              placeholder={t("library.search.placeholder")}
              value={query}
            />
          </div>
          <div
            className="flex items-center gap-1"
            role="group"
            aria-label={t("library.filter.label")}
          >
            {(["all", "movies", "shows"] as const).map((option) => (
              <Button
                aria-pressed={kind === option}
                key={option}
                onClick={() => {
                  setKind(option);
                }}
                size="sm"
                variant={kind === option ? "secondary" : "ghost"}
              >
                {t("library.filter." + option)}
              </Button>
            ))}
          </div>
        </div>

        {titles.loading ? null : titles.failed ? (
          <ErrorState body={t("library.unavailable.body")} title={t("library.unavailable.title")} />
        ) : titles.titles.length === 0 ? (
          <EmptyState body={t("library.empty.body")} size="page" title={t("library.empty.title")} />
        ) : shown.length === 0 ? (
          <EmptyState
            body={t("library.filter.none.body")}
            size="page"
            title={t("library.filter.none.title")}
          />
        ) : view === "grid" ? (
          /* **The height is load-bearing rather than cosmetic.** `Grid` scrolls itself, and a scroller
             with no height constraint grows to its content — which makes the "viewport" the whole grid
             and draws every tile, the failure `packages/ui`'s `Grid` exists to prevent. A
             window-relative bound keeps a wall of posters readable on a short window without letting it
             swallow the queue and the rule list below it on a tall one. */
          <div className="h-[min(60vh,40rem)] min-h-0" data-library-grid="">
            <Grid
              itemKey={(row) => row.id}
              items={shown}
              label={t("library.grid.label")}
              renderItem={(row) => (
                <TitleCard
                  {...downloadCardProps(marks, row.id, t, progress)}
                  detailsLabel={t("card.details")}
                  href={routePath(routeFor(row))}
                  meta={tileMeta(row, t)}
                  onNavigate={() => {
                    onNavigate(routeFor(row));
                  }}
                  // The core resolved this against the cache for every row, which is the whole reason
                  // the poster travels on the list read (AGENTS R7): a tile cannot hash a cache key.
                  poster={imageUrl(row.poster)}
                  watchlistLabel={t("card.watchlisted")}
                  watchlisted={watchlisted?.has(row.id) ?? false}
                  // **Not lifted, and that is the grid's geometry rather than a preference.** A grid's
                  // columns are derived to fit the measured width, so it has no room to reserve for a 30 %
                  // enlargement — and a scroller clips on both axes, so scaling anyway cut the outer tiles
                  // off at both sides. The panel reveals inside the tile instead.
                  lift={false}
                  // **Named, unlike a rail's tile.** A provider's poster identifies its title and a captured
                  // frame does not — and a library of frames is exactly the wall a reader cannot read.
                  showTitle
                  size="lg"
                  title={row.name}
                />
              )}
              rowHeight={TILE_ROW_HEIGHT}
              tileWidth={TILE_WIDTH}
            />
          </div>
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
              columns={queueColumnsOf(
                t,
                (file) => void unmatched.match(file),
                setChoosing,
                unmatched.busy,
              )}
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
            columns={ruleColumnsOf(t, setForgetting, rules.busy)}
            rowKey={(row) => row.id}
            rows={rules.rules}
          />
        </Section>
      )}

      {choosing === null ? null : (
        <TitlePicker
          fileName={choosing.name}
          onChoose={(title: TitleMatchDto) => {
            void unmatched.matchAs(choosing, title.id);
            setChoosing(null);
          }}
          onClose={() => {
            setChoosing(null);
          }}
        />
      )}

      {forgetting === null ? null : (
        <Dialog
          closeLabel={t("common.close")}
          description={t("library.rules.confirm.body", { pattern: forgetting.pattern })}
          onClose={() => {
            setForgetting(null);
          }}
          title={t("library.rules.confirm.title")}
        >
          <div className="flex justify-end gap-2">
            <Button
              onClick={() => {
                setForgetting(null);
              }}
              size="sm"
              variant="ghost"
            >
              {t("common.cancel")}
            </Button>
            <Button
              onClick={() => {
                void rules.forget(forgetting.id);
                setForgetting(null);
              }}
              size="sm"
              variant="danger"
            >
              {t("library.rules.forget")}
            </Button>
          </div>
        </Dialog>
      )}
    </Screen>
  );
}
