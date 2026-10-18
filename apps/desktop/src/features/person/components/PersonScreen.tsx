import type { ReactElement } from "react";
import { useTranslation } from "react-i18next";
import { EmptyState, ErrorState, Grid, Screen, Section, TitleCard } from "@peregrine/ui";
import { routePath } from "../../../app/routes.js";
import { imageUrl } from "../../../lib/media.js";
import { usePerson } from "../hooks/usePerson.js";
import type { Route } from "../../../app/routes.js";

/** One person's local filmography: the titles in this library that credit them. */
export function PersonScreen({
  backLabel,
  id,
  onBack,
  onNavigate,
}: {
  backLabel: string;
  id: number;
  onBack: (() => void) | undefined;
  onNavigate: (route: Route) => void;
}): ReactElement {
  const { t } = useTranslation();
  const person = usePerson(id);

  return (
    <Screen backLabel={backLabel} onBack={onBack} title={person.name ?? t("person.title")}>
      <Section
        description={t("person.description", { count: person.titles.length })}
        title={person.name ?? t("person.title")}
      >
        <div className="flex items-center gap-4" data-person-header="">
          {person.profile === null ? (
            <span
              aria-hidden="true"
              className="flex size-20 items-center justify-center rounded-full bg-surface-2 text-title font-medium text-text-secondary"
            >
              {initials(person.name ?? "")}
            </span>
          ) : (
            <img
              alt=""
              className="size-20 rounded-full object-cover"
              src={imageUrl(person.profile) ?? undefined}
            />
          )}
          <p className="text-body text-text-secondary">{t("person.local")}</p>
        </div>

        {person.loading ? (
          <p className="text-caption text-text-tertiary" role="status">
            {t("person.loading")}
          </p>
        ) : person.failed !== null ? (
          <ErrorState body={t("person.failed.body")} title={t("person.failed.title")} />
        ) : person.titles.length === 0 ? (
          <EmptyState body={t("person.empty.body")} size="page" title={t("person.empty.title")} />
        ) : (
          <div className="h-[min(60vh,40rem)] min-h-0">
            <Grid
              itemKey={(row) => row.id}
              items={person.titles}
              label={t("person.grid.label")}
              rowHeight={276}
              tileWidth={176}
              renderItem={(row) => (
                <TitleCard
                  detailsLabel={t("card.details")}
                  href={routePath({
                    kind: "title",
                    id: row.id,
                    titleKind: row.kind === "shows" ? "show" : "movie",
                  })}
                  meta={t("library.tiles.items", { count: row.items })}
                  onNavigate={() => {
                    onNavigate({
                      kind: "title",
                      id: row.id,
                      titleKind: row.kind === "shows" ? "show" : "movie",
                    });
                  }}
                  poster={imageUrl(row.poster)}
                  size="lg"
                  title={row.name}
                />
              )}
            />
          </div>
        )}
      </Section>
    </Screen>
  );
}

function initials(name: string): string {
  return name
    .trim()
    .split(/\s+/u)
    .slice(0, 2)
    .map((part) => part.charAt(0).toUpperCase())
    .join("");
}
