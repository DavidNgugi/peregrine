/**
 * The seasons of a show, as the row of chips that chooses between them.
 *
 * **Chips rather than a `<select>`, and the kit is why.** `packages/ui` has no select primitive, and the
 * Browse screen's kind switch is the pattern this copies: a `role="group"` of buttons with
 * `aria-pressed`, which is a single tab stop the arrow keys walk and a state a screen reader announces.
 * A native `<select>` styled to look like the rest of the kit would be a second control idiom for the
 * same decision, and the seasons of a show are few enough that seeing them all is the better answer.
 *
 * **Each chip says the season's number and, when the provider named it, nothing else.** The provider's
 * own season name is often just "Season 4" repeated, so the label is built from the number and the name
 * is used only when it says something the number does not — which is what makes "Specials" work, since
 * season zero's name is the only thing that identifies it.
 */

import type { SeasonDto } from "@peregrine/ipc-client";
import type { ReactElement } from "react";
import { useTranslation } from "react-i18next";

import { seasonLabel } from "./season-label.js";

export interface SeasonSwitcherProps {
  readonly seasons: readonly SeasonDto[];
  /** The season on screen, which is the one chip drawn as pressed. */
  readonly current: number;
  readonly onChoose: (seasonNumber: number) => void;
  /** Seasons with a future download schedule. */
  readonly scheduled?: ReadonlySet<number> | undefined;
}

/** The season chips. */
export function SeasonSwitcher({
  seasons,
  current,
  onChoose,
  scheduled,
}: SeasonSwitcherProps): ReactElement {
  const { t } = useTranslation();

  return (
    <div
      aria-label={t("title.season.label")}
      className="flex flex-wrap items-center gap-1 rounded-full border border-border-subtle bg-surface-0/70 p-1"
      role="group"
    >
      {seasons.map((season) => {
        const pressed = season.seasonNumber === current;
        return (
          <button
            aria-pressed={pressed}
            className={
              "rounded-full px-3 py-1 text-caption focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-focus-ring " +
              (pressed
                ? "bg-surface-2 text-text-primary"
                : "text-text-secondary hover:bg-surface-2/60")
            }
            key={season.seasonNumber}
            onClick={() => {
              onChoose(season.seasonNumber);
            }}
            type="button"
          >
            {scheduled?.has(season.seasonNumber)
              ? `${seasonLabel(season, t)} · ${t("title.episode.scheduled")}`
              : seasonLabel(season, t)}
          </button>
        );
      })}
    </div>
  );
}
