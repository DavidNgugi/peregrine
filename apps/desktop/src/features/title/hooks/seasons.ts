/**
 * The season switcher's own rules, as functions rather than as effects.
 *
 * **This file exists because a rule with no test is a rule nobody has run.** This repository's frontend
 * harness renders to static markup in a Node environment with no DOM (`vitest.config.ts`), so a hook's
 * effects never run in a test — which is why `rails-follow.ts`, `refresh-follow.ts`, and `paged-list.ts`
 * hold the decisions their hooks would otherwise bury. The two below are what a switcher turns on, and
 * both have a wrong answer that looks plausible on screen.
 */

import type { SeasonDto } from "@peregrine/ipc-client";

/**
 * The season a title opens on, or `None` when it has none to open.
 *
 * **Not the lowest number, and that is the whole reason this is a function.** Season zero is a show's
 * specials — the schema says so, and a special is an extra rather than the first thing to watch — so a
 * show that has one would open on its bonus disc. The earliest *numbered* season is the answer, and a
 * show with nothing but specials falls back to them rather than to an empty page.
 */
export function defaultSeason(seasons: readonly SeasonDto[]): SeasonDto | null {
  if (seasons.length === 0) {
    return null;
  }
  const numbered = seasons.filter((season) => season.seasonNumber !== 0);
  // Each list is already in number order because the read orders it, so the first of either is the
  // earliest of that kind rather than whichever the provider happened to mention first.
  return numbered[0] ?? seasons[0] ?? null;
}

/**
 * The season a *re-read* of the list should open on.
 *
 * **A re-read is not a navigation.** The seasons are read again when the refresh this screen asked
 * for lands, and by then the reader may be on season four; opening the read's default would move
 * them back to season one under a heading they were already reading. The season on screen is kept
 * when the new list still has it, and a list that no longer holds it — a title correction, a
 * provider that dropped a season — falls back to `defaultSeason` rather than to a number nothing
 * answers.
 */
export function seasonAfterRead(
  seasons: readonly SeasonDto[],
  current: number | null,
  preferred: number | null = null,
): number | null {
  if (current !== null && seasons.some((season) => season.seasonNumber === current)) {
    return current;
  }
  // The episode the account last touched is a better opening than the earliest season: a viewer
  // who finished season one belongs on season two, not back at the season they completed.
  if (preferred !== null && seasons.some((season) => season.seasonNumber === preferred)) {
    return preferred;
  }
  return defaultSeason(seasons)?.seasonNumber ?? null;
}

/**
 * How an episode's availability reads, from the counts the core sent.
 *
 * `AGENTS R7` puts business logic in Rust and the counts *are* the core's; what happens here is the
 * projection of four numbers onto the chip vocabulary a card already has, which is the same kind of
 * decision as turning bytes into a size label. The three cases the numbers can be in are the three that
 * mean something different to a reader:
 *
 * - **every file the library recorded is still there** — `available`, which the card draws no chip for
 *   because a poster for something you have is not a state worth a badge;
 * - **some are missing** — `partial`, which is the one a reader needs told, because the episode looks
 *   present in a listing and fails when played;
 * - **the library holds nothing** — `wanted`, which is a chip rather than silence because an episode
 *   with no file at all is the difference between a season that is short and one that was never
 *   acquired.
 *
 * An episode whose every file is gone is `missing` rather than `partial`: nothing is playable, and
 * "partly here" would be the wrong thing to say about it.
 */
export function episodeAvailability(row: {
  readonly items: number;
  readonly files: number;
  readonly missingFiles: number;
}): "available" | "partial" | "missing" | "wanted" {
  if (row.items === 0) {
    return "wanted";
  }
  if (row.missingFiles === 0) {
    return "available";
  }
  return row.missingFiles >= row.files ? "missing" : "partial";
}
