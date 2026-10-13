import { Button, EmptyState, ErrorState, Link, Screen, TextInput } from "@peregrine/ui";
import type { TitleMatchDto } from "@peregrine/ipc-client";
import { useState } from "react";
import type { ReactElement } from "react";
import { useTranslation } from "react-i18next";

import { routePath, type Route } from "../../../app/routes.js";
import { useDiscoverSearch } from "../hooks/useDiscoverSearch.js";

/**
 * Discover: search the configured providers, and show what the catalogue then holds.
 *
 * What comes back is a *title the catalogue owns*, not a provider's row — the core writes the answer
 * before this screen sees it (`PG-E6-04`), which is why a result can be opened and can survive the
 * provider going away.
 *
 * Search is submitted rather than typed: it leaves the machine, and a request per keystroke is a
 * request per keystroke against someone else's API (§9). The three states are kept apart, so a failed
 * search never looks like a search that found nothing — the second is a real answer about the
 * catalogue and the first is not an answer at all.
 */
export function DiscoverScreen({
  onNavigate,
}: {
  readonly onNavigate: (route: Route) => void;
}): ReactElement {
  const { t } = useTranslation();
  const { failed, results, searched, searching, search } = useDiscoverSearch();
  const [query, setQuery] = useState("");

  const routeFor = (row: TitleMatchDto): Route => ({
    id: row.id,
    kind: "title",
    titleKind: row.kind === "movies" ? "movie" : "show",
  });

  return (
    <Screen title={t("discover.title")}>
      <form
        className="flex items-end gap-2"
        onSubmit={(event) => {
          event.preventDefault();
          void search(query);
        }}
      >
        <TextInput
          className="max-w-md flex-1"
          label={t("discover.search.label")}
          onChange={(event) => {
            setQuery(event.target.value);
          }}
          placeholder={t("discover.search.placeholder")}
          value={query}
        />
        <Button disabled={searching || query.trim() === ""} type="submit">
          {t("discover.search.action")}
        </Button>
      </form>

      {/* Announced rather than drawn as a bar: there is no completion to show for a request whose
          length nobody knows, and a determinate bar would be a number invented to fill a shape. */}
      {searching ? (
        <p className="text-body text-text-secondary" role="status">
          {t("discover.search.searching")}
        </p>
      ) : null}

      {!searching && failed ? (
        <ErrorState body={t("discover.failed.body")} title={t("discover.failed.title")} />
      ) : null}

      {!searching && !failed && results.length > 0 ? (
        <ul aria-busy={searching} className="flex flex-col gap-1">
          {results.map((row) => (
            <li key={row.id}>
              {/* A link, because it is one: it says where it goes, and a keyboard user opens it with
                  `Enter` rather than activating a button. */}
              <Link
                href={routePath(routeFor(row))}
                onNavigate={() => {
                  onNavigate(routeFor(row));
                }}
              >
                {row.name}
              </Link>
              <span className="ml-2 text-caption text-text-tertiary">
                {row.kind === "movies" ? t("discover.result.movie") : t("discover.result.show")}
              </span>
            </li>
          ))}
        </ul>
      ) : null}

      {!searching && !failed && searched && results.length === 0 ? (
        <EmptyState body={t("discover.none.body")} title={t("discover.none.title")} />
      ) : null}

      {!searched && !searching && !failed ? (
        <EmptyState
          body={t("discover.empty.body")}
          className="m-auto max-w-md items-center text-center"
          size="page"
          title={t("discover.empty.title")}
        />
      ) : null}
    </Screen>
  );
}
