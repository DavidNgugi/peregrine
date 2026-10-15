/**
 * What one season chip says.
 *
 * A function rather than a line inside the chip, for the reason every other display rule in this feature
 * is one: the harness cannot run a component's effects and a rule that lives in a `map` body has no test
 * that would notice it changing. It also keeps the `t` call out of the render, which is where the
 * pluralisation rules would otherwise be written twice.
 */

import type { SeasonDto } from "@peregrine/ipc-client";
import type { useTranslation } from "react-i18next";

/** What `useTranslation` hands a component. */
type Translate = ReturnType<typeof useTranslation>["t"];

/**
 * The chip's word for one season.
 *
 * **The provider's name wins only when it says something the number does not.** Most providers name a
 * season `Season 4`, which is the number with a word in front of it, and a chip that printed both would
 * read "4 Season 4". Season zero is the case the name exists for: it is a show's specials, the provider
 * calls it that, and `0` is not a thing to put on a chip.
 */
export function seasonLabel(season: SeasonDto, t: Translate): string {
  const name = season.name?.trim() ?? "";
  if (season.seasonNumber === 0) {
    return name === "" ? t("title.season.specials") : name;
  }
  const generic = name === "" || name.toLowerCase() === `season ${String(season.seasonNumber)}`;
  return generic ? t("title.season.number", { number: season.seasonNumber }) : name;
}
